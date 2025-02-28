//! Simple plotting library.

use std::{cell::Cell, ops::RangeInclusive, rc::Rc};

use crate::*;
use epaint::ahash::AHashSet;
use epaint::color::Hsva;
use epaint::util::FloatOrd;

use items::PlotItem;
use legend::LegendWidget;
use transform::ScreenTransform;

pub use items::{
    Arrows, Bar, BarChart, BoxElem, BoxPlot, BoxSpread, HLine, Line, LineStyle, MarkerShape,
    Orientation, PlotImage, Points, Polygon, Text, VLine, Value, Values,
};
pub use legend::{Corner, Legend};
pub use transform::PlotBounds;

mod items;
mod legend;
mod transform;

type LabelFormatterFn = dyn Fn(&str, &Value) -> String;
type LabelFormatter = Option<Box<LabelFormatterFn>>;
type AxisFormatterFn = dyn Fn(f64, &RangeInclusive<f64>) -> String;
type AxisFormatter = Option<Box<AxisFormatterFn>>;

type GridSpacerFn = dyn Fn(GridInput) -> Vec<GridMark>;
type GridSpacer = Box<GridSpacerFn>;

/// Specifies the coordinates formatting when passed to [`Plot::coordinates_formatter`].
pub struct CoordinatesFormatter {
    function: Box<dyn Fn(&Value, &PlotBounds) -> String>,
}

impl CoordinatesFormatter {
    /// Create a new formatter based on the pointer coordinate and the plot bounds.
    pub fn new(function: impl Fn(&Value, &PlotBounds) -> String + 'static) -> Self {
        Self {
            function: Box::new(function),
        }
    }

    /// Show a fixed number of decimal places.
    pub fn with_decimals(num_decimals: usize) -> Self {
        Self {
            function: Box::new(move |value, _| {
                format!("x: {:.d$}\ny: {:.d$}", value.x, value.y, d = num_decimals)
            }),
        }
    }

    fn format(&self, value: &Value, bounds: &PlotBounds) -> String {
        (self.function)(value, bounds)
    }
}

impl Default for CoordinatesFormatter {
    fn default() -> Self {
        Self::with_decimals(3)
    }
}

// ----------------------------------------------------------------------------

const MIN_LINE_SPACING_IN_POINTS: f64 = 6.0; // TODO: large enough for a wide label

/// Information about the plot that has to persist between frames.
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[derive(Clone)]
struct PlotMemory {
    auto_bounds: bool,
    hovered_entry: Option<String>,
    hidden_items: AHashSet<String>,
    min_auto_bounds: PlotBounds,
    last_screen_transform: ScreenTransform,
    /// Allows to remember the first click position when performing a boxed zoom
    last_click_pos_for_zoom: Option<Pos2>,
}

impl PlotMemory {
    pub fn load(ctx: &Context, id: Id) -> Option<Self> {
        ctx.data().get_persisted(id)
    }

    pub fn store(self, ctx: &Context, id: Id) {
        ctx.data().insert_persisted(id, self);
    }
}

// ----------------------------------------------------------------------------

/// Defines how multiple plots share the same range for one or both of their axes. Can be added while building
/// a plot with [`Plot::link_axis`]. Contains an internal state, meaning that this object should be stored by
/// the user between frames.
#[derive(Clone, PartialEq)]
pub struct LinkedAxisGroup {
    pub(crate) link_x: bool,
    pub(crate) link_y: bool,
    pub(crate) bounds: Rc<Cell<Option<PlotBounds>>>,
}

impl LinkedAxisGroup {
    pub fn new(link_x: bool, link_y: bool) -> Self {
        Self {
            link_x,
            link_y,
            bounds: Rc::new(Cell::new(None)),
        }
    }

    /// Only link the x-axis.
    pub fn x() -> Self {
        Self::new(true, false)
    }

    /// Only link the y-axis.
    pub fn y() -> Self {
        Self::new(false, true)
    }

    /// Link both axes. Note that this still respects the aspect ratio of the individual plots.
    pub fn both() -> Self {
        Self::new(true, true)
    }

    /// Change whether the x-axis is linked for this group. Using this after plots in this group have been
    /// drawn in this frame already may lead to unexpected results.
    pub fn set_link_x(&mut self, link: bool) {
        self.link_x = link;
    }

    /// Change whether the y-axis is linked for this group. Using this after plots in this group have been
    /// drawn in this frame already may lead to unexpected results.
    pub fn set_link_y(&mut self, link: bool) {
        self.link_y = link;
    }

    fn get(&self) -> Option<PlotBounds> {
        self.bounds.get()
    }

    fn set(&self, bounds: PlotBounds) {
        self.bounds.set(Some(bounds));
    }
}

// ----------------------------------------------------------------------------

/// A 2D plot, e.g. a graph of a function.
///
/// [`Plot`] supports multiple lines and points.
///
/// ```
/// # egui::__run_test_ui(|ui| {
/// use egui::plot::{Line, Plot, Value, Values};
/// let sin = (0..1000).map(|i| {
///     let x = i as f64 * 0.01;
///     Value::new(x, x.sin())
/// });
/// let line = Line::new(Values::from_values_iter(sin));
/// Plot::new("my_plot").view_aspect(2.0).show(ui, |plot_ui| plot_ui.line(line));
/// # });
/// ```
pub struct Plot {
    id_source: Id,

    center_x_axis: bool,
    center_y_axis: bool,
    allow_zoom: bool,
    allow_drag: bool,
    allow_scroll: bool,
    min_auto_bounds: PlotBounds,
    margin_fraction: Vec2,
    allow_boxed_zoom: bool,
    boxed_zoom_pointer_button: PointerButton,
    linked_axes: Option<LinkedAxisGroup>,

    min_size: Vec2,
    width: Option<f32>,
    height: Option<f32>,
    data_aspect: Option<f32>,
    view_aspect: Option<f32>,

    show_x: bool,
    show_y: bool,
    label_formatter: LabelFormatter,
    coordinates_formatter: Option<(Corner, CoordinatesFormatter)>,
    axis_formatters: [AxisFormatter; 2],
    legend_config: Option<Legend>,
    show_background: bool,
    show_axes: [bool; 2],
    grid_spacers: [GridSpacer; 2],
}

impl Plot {
    /// Give a unique id for each plot within the same [`Ui`].
    pub fn new(id_source: impl std::hash::Hash) -> Self {
        Self {
            id_source: Id::new(id_source),

            center_x_axis: false,
            center_y_axis: false,
            allow_zoom: true,
            allow_drag: true,
            allow_scroll: true,
            min_auto_bounds: PlotBounds::NOTHING,
            margin_fraction: Vec2::splat(0.05),
            allow_boxed_zoom: true,
            boxed_zoom_pointer_button: PointerButton::Secondary,
            linked_axes: None,

            min_size: Vec2::splat(64.0),
            width: None,
            height: None,
            data_aspect: None,
            view_aspect: None,

            show_x: true,
            show_y: true,
            label_formatter: None,
            coordinates_formatter: None,
            axis_formatters: [None, None], // [None; 2] requires Copy
            legend_config: None,
            show_background: true,
            show_axes: [true; 2],
            grid_spacers: [log_grid_spacer(10), log_grid_spacer(10)],
        }
    }

    /// width / height ratio of the data.
    /// For instance, it can be useful to set this to `1.0` for when the two axes show the same
    /// unit.
    /// By default the plot window's aspect ratio is used.
    pub fn data_aspect(mut self, data_aspect: f32) -> Self {
        self.data_aspect = Some(data_aspect);
        self
    }

    /// width / height ratio of the plot region.
    /// By default no fixed aspect ratio is set (and width/height will fill the ui it is in).
    pub fn view_aspect(mut self, view_aspect: f32) -> Self {
        self.view_aspect = Some(view_aspect);
        self
    }

    /// Width of plot. By default a plot will fill the ui it is in.
    /// If you set [`Self::view_aspect`], the width can be calculated from the height.
    pub fn width(mut self, width: f32) -> Self {
        self.min_size.x = width;
        self.width = Some(width);
        self
    }

    /// Height of plot. By default a plot will fill the ui it is in.
    /// If you set [`Self::view_aspect`], the height can be calculated from the width.
    pub fn height(mut self, height: f32) -> Self {
        self.min_size.y = height;
        self.height = Some(height);
        self
    }

    /// Minimum size of the plot view.
    pub fn min_size(mut self, min_size: Vec2) -> Self {
        self.min_size = min_size;
        self
    }

    /// Show the x-value (e.g. when hovering). Default: `true`.
    pub fn show_x(mut self, show_x: bool) -> Self {
        self.show_x = show_x;
        self
    }

    /// Show the y-value (e.g. when hovering). Default: `true`.
    pub fn show_y(mut self, show_y: bool) -> Self {
        self.show_y = show_y;
        self
    }

    /// Always keep the x-axis centered. Default: `false`.
    pub fn center_x_axis(mut self, on: bool) -> Self {
        self.center_x_axis = on;
        self
    }

    /// Always keep the y-axis centered. Default: `false`.
    pub fn center_y_axis(mut self, on: bool) -> Self {
        self.center_y_axis = on;
        self
    }

    /// Whether to allow zooming in the plot. Default: `true`.
    pub fn allow_zoom(mut self, on: bool) -> Self {
        self.allow_zoom = on;
        self
    }

    /// Whether to allow scrolling in the plot. Default: `true`.
    pub fn allow_scroll(mut self, on: bool) -> Self {
        self.allow_scroll = on;
        self
    }

    /// Set the side margin as a fraction of the plot size.
    ///
    /// For instance, a value of `0.1` will add 10% space on both sides.
    pub fn set_margin_fraction(mut self, margin_fraction: Vec2) -> Self {
        self.margin_fraction = margin_fraction;
        self
    }

    /// Whether to allow zooming in the plot by dragging out a box with the secondary mouse button.
    ///
    /// Default: `true`.
    pub fn allow_boxed_zoom(mut self, on: bool) -> Self {
        self.allow_boxed_zoom = on;
        self
    }

    /// Config the button pointer to use for boxed zooming. Default: [`Secondary`](PointerButton::Secondary)
    pub fn boxed_zoom_pointer_button(mut self, boxed_zoom_pointer_button: PointerButton) -> Self {
        self.boxed_zoom_pointer_button = boxed_zoom_pointer_button;
        self
    }

    /// Whether to allow dragging in the plot to move the bounds. Default: `true`.
    pub fn allow_drag(mut self, on: bool) -> Self {
        self.allow_drag = on;
        self
    }

    /// Provide a function to customize the on-hovel label for the x and y axis
    ///
    /// ```
    /// # egui::__run_test_ui(|ui| {
    /// use egui::plot::{Line, Plot, Value, Values};
    /// let sin = (0..1000).map(|i| {
    ///     let x = i as f64 * 0.01;
    ///     Value::new(x, x.sin())
    /// });
    /// let line = Line::new(Values::from_values_iter(sin));
    /// Plot::new("my_plot").view_aspect(2.0)
    /// .label_formatter(|name, value| {
    ///     if !name.is_empty() {
    ///         format!("{}: {:.*}%", name, 1, value.y)
    ///     } else {
    ///         "".to_string()
    ///     }
    /// })
    /// .show(ui, |plot_ui| plot_ui.line(line));
    /// # });
    /// ```
    pub fn label_formatter(
        mut self,
        label_formatter: impl Fn(&str, &Value) -> String + 'static,
    ) -> Self {
        self.label_formatter = Some(Box::new(label_formatter));
        self
    }

    /// Show the pointer coordinates in the plot.
    pub fn coordinates_formatter(
        mut self,
        position: Corner,
        formatter: CoordinatesFormatter,
    ) -> Self {
        self.coordinates_formatter = Some((position, formatter));
        self
    }

    /// Provide a function to customize the labels for the X axis based on the current visible value range.
    ///
    /// This is useful for custom input domains, e.g. date/time.
    ///
    /// If axis labels should not appear for certain values or beyond a certain zoom/resolution,
    /// the formatter function can return empty strings. This is also useful if your domain is
    /// discrete (e.g. only full days in a calendar).
    pub fn x_axis_formatter(
        mut self,
        func: impl Fn(f64, &RangeInclusive<f64>) -> String + 'static,
    ) -> Self {
        self.axis_formatters[0] = Some(Box::new(func));
        self
    }

    /// Provide a function to customize the labels for the Y axis based on the current value range.
    ///
    /// This is useful for custom value representation, e.g. percentage or units.
    ///
    /// If axis labels should not appear for certain values or beyond a certain zoom/resolution,
    /// the formatter function can return empty strings. This is also useful if your Y values are
    /// discrete (e.g. only integers).
    pub fn y_axis_formatter(
        mut self,
        func: impl Fn(f64, &RangeInclusive<f64>) -> String + 'static,
    ) -> Self {
        self.axis_formatters[1] = Some(Box::new(func));
        self
    }

    /// Configure how the grid in the background is spaced apart along the X axis.
    ///
    /// Default is a log-10 grid, i.e. every plot unit is divided into 10 other units.
    ///
    /// The function has this signature:
    /// ```ignore
    /// fn get_step_sizes(input: GridInput) -> Vec<GridMark>;
    /// ```
    ///
    /// This function should return all marks along the visible range of the X axis.
    /// `step_size` also determines how thick/faint each line is drawn.
    /// For example, if x = 80..=230 is visible and you want big marks at steps of
    /// 100 and small ones at 25, you can return:
    /// ```no_run
    /// # use egui::plot::GridMark;
    /// vec![
    ///    // 100s
    ///    GridMark { value: 100.0, step_size: 100.0 },
    ///    GridMark { value: 200.0, step_size: 100.0 },
    ///
    ///    // 25s
    ///    GridMark { value: 125.0, step_size: 25.0 },
    ///    GridMark { value: 150.0, step_size: 25.0 },
    ///    GridMark { value: 175.0, step_size: 25.0 },
    ///    GridMark { value: 225.0, step_size: 25.0 },
    /// ];
    /// # ()
    /// ```
    ///
    /// There are helpers for common cases, see [`log_grid_spacer`] and [`uniform_grid_spacer`].
    pub fn x_grid_spacer(mut self, spacer: impl Fn(GridInput) -> Vec<GridMark> + 'static) -> Self {
        self.grid_spacers[0] = Box::new(spacer);
        self
    }

    /// Default is a log-10 grid, i.e. every plot unit is divided into 10 other units.
    ///
    /// See [`Self::x_grid_spacer`] for explanation.
    pub fn y_grid_spacer(mut self, spacer: impl Fn(GridInput) -> Vec<GridMark> + 'static) -> Self {
        self.grid_spacers[1] = Box::new(spacer);
        self
    }

    /// Expand bounds to include the given x value.
    /// For instance, to always show the y axis, call `plot.include_x(0.0)`.
    pub fn include_x(mut self, x: impl Into<f64>) -> Self {
        self.min_auto_bounds.extend_with_x(x.into());
        self
    }

    /// Expand bounds to include the given y value.
    /// For instance, to always show the x axis, call `plot.include_y(0.0)`.
    pub fn include_y(mut self, y: impl Into<f64>) -> Self {
        self.min_auto_bounds.extend_with_y(y.into());
        self
    }

    /// Show a legend including all named items.
    pub fn legend(mut self, legend: Legend) -> Self {
        self.legend_config = Some(legend);
        self
    }

    /// Whether or not to show the background [`Rect`].
    /// Can be useful to disable if the plot is overlaid over existing content.
    /// Default: `true`.
    pub fn show_background(mut self, show: bool) -> Self {
        self.show_background = show;
        self
    }

    /// Show the axes.
    /// Can be useful to disable if the plot is overlaid over an existing grid or content.
    /// Default: `[true; 2]`.
    pub fn show_axes(mut self, show: [bool; 2]) -> Self {
        self.show_axes = show;
        self
    }

    /// Add a [`LinkedAxisGroup`] so that this plot will share the bounds with other plots that have this
    /// group assigned. A plot cannot belong to more than one group.
    pub fn link_axis(mut self, group: LinkedAxisGroup) -> Self {
        self.linked_axes = Some(group);
        self
    }

    /// Interact with and add items to the plot and finally draw it.
    pub fn show<R>(self, ui: &mut Ui, build_fn: impl FnOnce(&mut PlotUi) -> R) -> InnerResponse<R> {
        let Self {
            id_source,
            center_x_axis,
            center_y_axis,
            allow_zoom,
            allow_scroll,
            allow_drag,
            allow_boxed_zoom,
            boxed_zoom_pointer_button: boxed_zoom_pointer,
            min_auto_bounds,
            margin_fraction,
            width,
            height,
            min_size,
            data_aspect,
            view_aspect,
            mut show_x,
            mut show_y,
            label_formatter,
            coordinates_formatter,
            axis_formatters,
            legend_config,
            show_background,
            show_axes,
            linked_axes,
            grid_spacers,
        } = self;

        // Determine the size of the plot in the UI
        let size = {
            let width = width
                .unwrap_or_else(|| {
                    if let (Some(height), Some(aspect)) = (height, view_aspect) {
                        height * aspect
                    } else {
                        ui.available_size_before_wrap().x
                    }
                })
                .at_least(min_size.x);

            let height = height
                .unwrap_or_else(|| {
                    if let Some(aspect) = view_aspect {
                        width / aspect
                    } else {
                        ui.available_size_before_wrap().y
                    }
                })
                .at_least(min_size.y);
            vec2(width, height)
        };

        // Allocate the space.
        let (rect, response) = ui.allocate_exact_size(size, Sense::drag());

        // Load or initialize the memory.
        let plot_id = ui.make_persistent_id(id_source);
        ui.ctx().check_for_id_clash(plot_id, rect, "Plot");
        let mut memory = PlotMemory::load(ui.ctx(), plot_id).unwrap_or_else(|| PlotMemory {
            auto_bounds: !min_auto_bounds.is_valid(),
            hovered_entry: None,
            hidden_items: Default::default(),
            min_auto_bounds,
            last_screen_transform: ScreenTransform::new(
                rect,
                min_auto_bounds,
                center_x_axis,
                center_y_axis,
            ),
            last_click_pos_for_zoom: None,
        });

        // If the min bounds changed, recalculate everything.
        if min_auto_bounds != memory.min_auto_bounds {
            memory = PlotMemory {
                auto_bounds: !min_auto_bounds.is_valid(),
                hovered_entry: None,
                min_auto_bounds,
                ..memory
            };
            memory.clone().store(ui.ctx(), plot_id);
        }

        let PlotMemory {
            mut auto_bounds,
            mut hovered_entry,
            mut hidden_items,
            last_screen_transform,
            mut last_click_pos_for_zoom,
            ..
        } = memory;

        // Call the plot build function.
        let mut plot_ui = PlotUi {
            items: Vec::new(),
            next_auto_color_idx: 0,
            last_screen_transform,
            response,
            ctx: ui.ctx().clone(),
        };
        let inner = build_fn(&mut plot_ui);
        let PlotUi {
            mut items,
            mut response,
            last_screen_transform,
            ..
        } = plot_ui;

        // Background
        if show_background {
            ui.painter().with_clip_rect(rect).add(epaint::RectShape {
                rect,
                rounding: Rounding::same(2.0),
                fill: ui.visuals().extreme_bg_color,
                stroke: ui.visuals().widgets.noninteractive.bg_stroke,
            });
        }

        // --- Legend ---
        let legend = legend_config
            .and_then(|config| LegendWidget::try_new(rect, config, &items, &hidden_items));
        // Don't show hover cursor when hovering over legend.
        if hovered_entry.is_some() {
            show_x = false;
            show_y = false;
        }
        // Remove the deselected items.
        items.retain(|item| !hidden_items.contains(item.name()));
        // Highlight the hovered items.
        if let Some(hovered_name) = &hovered_entry {
            items
                .iter_mut()
                .filter(|entry| entry.name() == hovered_name)
                .for_each(|entry| entry.highlight());
        }
        // Move highlighted items to front.
        items.sort_by_key(|item| item.highlighted());

        // --- Bound computation ---
        let mut bounds = *last_screen_transform.bounds();

        // Transfer the bounds from a link group.
        if let Some(axes) = linked_axes.as_ref() {
            if let Some(linked_bounds) = axes.get() {
                if axes.link_x {
                    bounds.min[0] = linked_bounds.min[0];
                    bounds.max[0] = linked_bounds.max[0];
                }
                if axes.link_y {
                    bounds.min[1] = linked_bounds.min[1];
                    bounds.max[1] = linked_bounds.max[1];
                }
                // Turn off auto bounds to keep it from overriding what we just set.
                auto_bounds = false;
            }
        }

        // Allow double clicking to reset to automatic bounds.
        auto_bounds |= response.double_clicked_by(PointerButton::Primary);

        // Set bounds automatically based on content.
        if auto_bounds || !bounds.is_valid() {
            bounds = min_auto_bounds;
            for item in &items {
                bounds.merge(&item.get_bounds());
            }
            bounds.add_relative_margin(margin_fraction);
        }

        let mut transform = ScreenTransform::new(rect, bounds, center_x_axis, center_y_axis);

        // Enforce equal aspect ratio.
        if let Some(data_aspect) = data_aspect {
            let preserve_y = linked_axes
                .as_ref()
                .map_or(false, |group| group.link_y && !group.link_x);
            transform.set_aspect(data_aspect as f64, preserve_y);
        }

        // Dragging
        if allow_drag && response.dragged_by(PointerButton::Primary) {
            response = response.on_hover_cursor(CursorIcon::Grabbing);
            transform.translate_bounds(-response.drag_delta());
            auto_bounds = false;
        }

        // Zooming
        let mut boxed_zoom_rect = None;
        if allow_boxed_zoom {
            // Save last click to allow boxed zooming
            if response.drag_started() && response.dragged_by(boxed_zoom_pointer) {
                // it would be best for egui that input has a memory of the last click pos because it's a common pattern
                last_click_pos_for_zoom = response.hover_pos();
            }
            let box_start_pos = last_click_pos_for_zoom;
            let box_end_pos = response.hover_pos();
            if let (Some(box_start_pos), Some(box_end_pos)) = (box_start_pos, box_end_pos) {
                // while dragging prepare a Shape and draw it later on top of the plot
                if response.dragged_by(boxed_zoom_pointer) {
                    response = response.on_hover_cursor(CursorIcon::ZoomIn);
                    let rect = epaint::Rect::from_two_pos(box_start_pos, box_end_pos);
                    boxed_zoom_rect = Some((
                        epaint::RectShape::stroke(
                            rect,
                            0.0,
                            epaint::Stroke::new(4., Color32::DARK_BLUE),
                        ), // Outer stroke
                        epaint::RectShape::stroke(
                            rect,
                            0.0,
                            epaint::Stroke::new(2., Color32::WHITE),
                        ), // Inner stroke
                    ));
                }
                // when the click is release perform the zoom
                if response.drag_released() {
                    let box_start_pos = transform.value_from_position(box_start_pos);
                    let box_end_pos = transform.value_from_position(box_end_pos);
                    let new_bounds = PlotBounds {
                        min: [box_start_pos.x, box_end_pos.y],
                        max: [box_end_pos.x, box_start_pos.y],
                    };
                    if new_bounds.is_valid() {
                        *transform.bounds_mut() = new_bounds;
                        auto_bounds = false;
                    } else {
                        auto_bounds = true;
                    }
                    // reset the boxed zoom state
                    last_click_pos_for_zoom = None;
                }
            }
        }

        if let Some(hover_pos) = response.hover_pos() {
            if allow_zoom {
                let zoom_factor = if data_aspect.is_some() {
                    Vec2::splat(ui.input().zoom_delta())
                } else {
                    ui.input().zoom_delta_2d()
                };
                if zoom_factor != Vec2::splat(1.0) {
                    transform.zoom(zoom_factor, hover_pos);
                    auto_bounds = false;
                }
            }
            if allow_scroll {
                let scroll_delta = ui.input().scroll_delta;
                if scroll_delta != Vec2::ZERO {
                    transform.translate_bounds(-scroll_delta);
                    auto_bounds = false;
                }
            }
        }

        // Initialize values from functions.
        for item in &mut items {
            item.initialize(transform.bounds().range_x());
        }

        let prepared = PreparedPlot {
            items,
            show_x,
            show_y,
            label_formatter,
            coordinates_formatter,
            axis_formatters,
            show_axes,
            transform: transform.clone(),
            grid_spacers,
        };
        prepared.ui(ui, &response);

        if let Some(boxed_zoom_rect) = boxed_zoom_rect {
            ui.painter().with_clip_rect(rect).add(boxed_zoom_rect.0);
            ui.painter().with_clip_rect(rect).add(boxed_zoom_rect.1);
        }

        if let Some(mut legend) = legend {
            ui.add(&mut legend);
            hidden_items = legend.get_hidden_items();
            hovered_entry = legend.get_hovered_entry_name();
        }

        if let Some(group) = linked_axes.as_ref() {
            group.set(*transform.bounds());
        }

        let memory = PlotMemory {
            auto_bounds,
            hovered_entry,
            hidden_items,
            min_auto_bounds,
            last_screen_transform: transform,
            last_click_pos_for_zoom,
        };
        memory.store(ui.ctx(), plot_id);

        let response = if show_x || show_y {
            response.on_hover_cursor(CursorIcon::Crosshair)
        } else {
            response
        };

        InnerResponse { inner, response }
    }
}

/// Provides methods to interact with a plot while building it. It is the single argument of the closure
/// provided to [`Plot::show`]. See [`Plot`] for an example of how to use it.
pub struct PlotUi {
    items: Vec<Box<dyn PlotItem>>,
    next_auto_color_idx: usize,
    last_screen_transform: ScreenTransform,
    response: Response,
    ctx: Context,
}

impl PlotUi {
    fn auto_color(&mut self) -> Color32 {
        let i = self.next_auto_color_idx;
        self.next_auto_color_idx += 1;
        let golden_ratio = (5.0_f32.sqrt() - 1.0) / 2.0; // 0.61803398875
        let h = i as f32 * golden_ratio;
        Hsva::new(h, 0.85, 0.5, 1.0).into() // TODO: OkLab or some other perspective color space
    }

    pub fn ctx(&self) -> &Context {
        &self.ctx
    }

    /// The plot bounds as they were in the last frame. If called on the first frame and the bounds were not
    /// further specified in the plot builder, this will return bounds centered on the origin. The bounds do
    /// not change until the plot is drawn.
    pub fn plot_bounds(&self) -> PlotBounds {
        *self.last_screen_transform.bounds()
    }

    /// Returns `true` if the plot area is currently hovered.
    pub fn plot_hovered(&self) -> bool {
        self.response.hovered()
    }

    /// Returns `true` if the plot was clicked by the primary button.
    pub fn plot_clicked(&self) -> bool {
        self.response.clicked()
    }

    /// The pointer position in plot coordinates. Independent of whether the pointer is in the plot area.
    pub fn pointer_coordinate(&self) -> Option<Value> {
        // We need to subtract the drag delta to keep in sync with the frame-delayed screen transform:
        let last_pos = self.ctx().input().pointer.latest_pos()? - self.response.drag_delta();
        let value = self.plot_from_screen(last_pos);
        Some(value)
    }

    /// The pointer drag delta in plot coordinates.
    pub fn pointer_coordinate_drag_delta(&self) -> Vec2 {
        let delta = self.response.drag_delta();
        let dp_dv = self.last_screen_transform.dpos_dvalue();
        Vec2::new(delta.x / dp_dv[0] as f32, delta.y / dp_dv[1] as f32)
    }

    /// Transform the plot coordinates to screen coordinates.
    pub fn screen_from_plot(&self, position: Value) -> Pos2 {
        self.last_screen_transform.position_from_value(&position)
    }

    /// Transform the screen coordinates to plot coordinates.
    pub fn plot_from_screen(&self, position: Pos2) -> Value {
        self.last_screen_transform.value_from_position(position)
    }

    /// Add a data line.
    pub fn line(&mut self, mut line: Line) {
        if line.series.is_empty() {
            return;
        };

        // Give the stroke an automatic color if no color has been assigned.
        if line.stroke.color == Color32::TRANSPARENT {
            line.stroke.color = self.auto_color();
        }
        self.items.push(Box::new(line));
    }

    /// Add a polygon. The polygon has to be convex.
    pub fn polygon(&mut self, mut polygon: Polygon) {
        if polygon.series.is_empty() {
            return;
        };

        // Give the stroke an automatic color if no color has been assigned.
        if polygon.stroke.color == Color32::TRANSPARENT {
            polygon.stroke.color = self.auto_color();
        }
        self.items.push(Box::new(polygon));
    }

    /// Add a text.
    pub fn text(&mut self, text: Text) {
        if text.text.is_empty() {
            return;
        };

        self.items.push(Box::new(text));
    }

    /// Add data points.
    pub fn points(&mut self, mut points: Points) {
        if points.series.is_empty() {
            return;
        };

        // Give the points an automatic color if no color has been assigned.
        if points.color == Color32::TRANSPARENT {
            points.color = self.auto_color();
        }
        self.items.push(Box::new(points));
    }

    /// Add arrows.
    pub fn arrows(&mut self, mut arrows: Arrows) {
        if arrows.origins.is_empty() || arrows.tips.is_empty() {
            return;
        };

        // Give the arrows an automatic color if no color has been assigned.
        if arrows.color == Color32::TRANSPARENT {
            arrows.color = self.auto_color();
        }
        self.items.push(Box::new(arrows));
    }

    /// Add an image.
    pub fn image(&mut self, image: PlotImage) {
        self.items.push(Box::new(image));
    }

    /// Add a horizontal line.
    /// Can be useful e.g. to show min/max bounds or similar.
    /// Always fills the full width of the plot.
    pub fn hline(&mut self, mut hline: HLine) {
        if hline.stroke.color == Color32::TRANSPARENT {
            hline.stroke.color = self.auto_color();
        }
        self.items.push(Box::new(hline));
    }

    /// Add a vertical line.
    /// Can be useful e.g. to show min/max bounds or similar.
    /// Always fills the full height of the plot.
    pub fn vline(&mut self, mut vline: VLine) {
        if vline.stroke.color == Color32::TRANSPARENT {
            vline.stroke.color = self.auto_color();
        }
        self.items.push(Box::new(vline));
    }

    /// Add a box plot diagram.
    pub fn box_plot(&mut self, mut box_plot: BoxPlot) {
        if box_plot.boxes.is_empty() {
            return;
        }

        // Give the elements an automatic color if no color has been assigned.
        if box_plot.default_color == Color32::TRANSPARENT {
            box_plot = box_plot.color(self.auto_color());
        }
        self.items.push(Box::new(box_plot));
    }

    /// Add a bar chart.
    pub fn bar_chart(&mut self, mut chart: BarChart) {
        if chart.bars.is_empty() {
            return;
        }

        // Give the elements an automatic color if no color has been assigned.
        if chart.default_color == Color32::TRANSPARENT {
            chart = chart.color(self.auto_color());
        }
        self.items.push(Box::new(chart));
    }
}

// ----------------------------------------------------------------------------
// Grid

/// Input for "grid spacer" functions.
///
/// See [`Plot::x_grid_spacer()`] and [`Plot::y_grid_spacer()`].
pub struct GridInput {
    /// Min/max of the visible data range (the values at the two edges of the plot,
    /// for the current axis).
    pub bounds: (f64, f64),

    /// Recommended (but not required) lower-bound on the step size returned by custom grid spacers.
    ///
    /// Computed as the ratio between the diagram's bounds (in plot coordinates) and the viewport
    /// (in frame/window coordinates), scaled up to represent the minimal possible step.
    pub base_step_size: f64,
}

/// One mark (horizontal or vertical line) in the background grid of a plot.
pub struct GridMark {
    /// X or Y value in the plot.
    pub value: f64,

    /// The (approximate) distance to the next value of same thickness.
    ///
    /// Determines how thick the grid line is painted. It's not important that `step_size`
    /// matches the difference between two `value`s precisely, but rather that grid marks of
    /// same thickness have same `step_size`. For example, months can have a different number
    /// of days, but consistently using a `step_size` of 30 days is a valid approximation.
    pub step_size: f64,
}

/// Recursively splits the grid into `base` subdivisions (e.g. 100, 10, 1).
///
/// The logarithmic base, expressing how many times each grid unit is subdivided.
/// 10 is a typical value, others are possible though.
pub fn log_grid_spacer(log_base: i64) -> GridSpacer {
    let log_base = log_base as f64;
    let get_step_sizes = move |input: GridInput| -> Vec<GridMark> {
        // The distance between two of the thinnest grid lines is "rounded" up
        // to the next-bigger power of base
        let smallest_visible_unit = next_power(input.base_step_size, log_base);

        let step_sizes = [
            smallest_visible_unit,
            smallest_visible_unit * log_base,
            smallest_visible_unit * log_base * log_base,
        ];

        generate_marks(step_sizes, input.bounds)
    };

    Box::new(get_step_sizes)
}

/// Splits the grid into uniform-sized spacings (e.g. 100, 25, 1).
///
/// This function should return 3 positive step sizes, designating where the lines in the grid are drawn.
/// Lines are thicker for larger step sizes. Ordering of returned value is irrelevant.
///
/// Why only 3 step sizes? Three is the number of different line thicknesses that egui typically uses in the grid.
/// Ideally, those 3 are not hardcoded values, but depend on the visible range (accessible through `GridInput`).
pub fn uniform_grid_spacer(spacer: impl Fn(GridInput) -> [f64; 3] + 'static) -> GridSpacer {
    let get_marks = move |input: GridInput| -> Vec<GridMark> {
        let bounds = input.bounds;
        let step_sizes = spacer(input);
        generate_marks(step_sizes, bounds)
    };

    Box::new(get_marks)
}

// ----------------------------------------------------------------------------

struct PreparedPlot {
    items: Vec<Box<dyn PlotItem>>,
    show_x: bool,
    show_y: bool,
    label_formatter: LabelFormatter,
    coordinates_formatter: Option<(Corner, CoordinatesFormatter)>,
    axis_formatters: [AxisFormatter; 2],
    show_axes: [bool; 2],
    transform: ScreenTransform,
    grid_spacers: [GridSpacer; 2],
}

impl PreparedPlot {
    fn ui(self, ui: &mut Ui, response: &Response) {
        let mut shapes = Vec::new();

        for d in 0..2 {
            if self.show_axes[d] {
                self.paint_axis(ui, d, &mut shapes);
            }
        }

        let transform = &self.transform;

        let mut plot_ui = ui.child_ui(*transform.frame(), Layout::default());
        plot_ui.set_clip_rect(*transform.frame());
        for item in &self.items {
            item.get_shapes(&mut plot_ui, transform, &mut shapes);
        }

        if let Some(pointer) = response.hover_pos() {
            self.hover(ui, pointer, &mut shapes);
        }

        let painter = ui.painter().with_clip_rect(*transform.frame());
        painter.extend(shapes);

        if let Some((corner, formatter)) = self.coordinates_formatter.as_ref() {
            if let Some(pointer) = response.hover_pos() {
                let font_id = TextStyle::Monospace.resolve(ui.style());
                let coordinate = transform.value_from_position(pointer);
                let text = formatter.format(&coordinate, transform.bounds());
                let padded_frame = transform.frame().shrink(4.0);
                let (anchor, position) = match corner {
                    Corner::LeftTop => (Align2::LEFT_TOP, padded_frame.left_top()),
                    Corner::RightTop => (Align2::RIGHT_TOP, padded_frame.right_top()),
                    Corner::LeftBottom => (Align2::LEFT_BOTTOM, padded_frame.left_bottom()),
                    Corner::RightBottom => (Align2::RIGHT_BOTTOM, padded_frame.right_bottom()),
                };
                painter.text(position, anchor, text, font_id, ui.visuals().text_color());
            }
        }
    }

    fn paint_axis(&self, ui: &Ui, axis: usize, shapes: &mut Vec<Shape>) {
        let Self {
            transform,
            axis_formatters,
            grid_spacers,
            ..
        } = self;

        let bounds = transform.bounds();
        let axis_range = match axis {
            0 => bounds.range_x(),
            1 => bounds.range_y(),
            _ => panic!("Axis {} does not exist.", axis),
        };

        let font_id = TextStyle::Body.resolve(ui.style());

        // Where on the cross-dimension to show the label values
        let bounds = transform.bounds();
        let value_cross = 0.0_f64.clamp(bounds.min[1 - axis], bounds.max[1 - axis]);

        let input = GridInput {
            bounds: (bounds.min[axis], bounds.max[axis]),
            base_step_size: transform.dvalue_dpos()[axis] * MIN_LINE_SPACING_IN_POINTS,
        };
        let steps = (grid_spacers[axis])(input);

        for step in steps {
            let value_main = step.value;

            let value = if axis == 0 {
                Value::new(value_main, value_cross)
            } else {
                Value::new(value_cross, value_main)
            };

            let pos_in_gui = transform.position_from_value(&value);
            let spacing_in_points = (transform.dpos_dvalue()[axis] * step.step_size).abs() as f32;

            let line_alpha = remap_clamp(
                spacing_in_points,
                (MIN_LINE_SPACING_IN_POINTS as f32)..=300.0,
                0.0..=0.15,
            );

            if line_alpha > 0.0 {
                let line_color = color_from_alpha(ui, line_alpha);

                let mut p0 = pos_in_gui;
                let mut p1 = pos_in_gui;
                p0[1 - axis] = transform.frame().min[1 - axis];
                p1[1 - axis] = transform.frame().max[1 - axis];
                shapes.push(Shape::line_segment([p0, p1], Stroke::new(1.0, line_color)));
            }

            let text_alpha = remap_clamp(spacing_in_points, 40.0..=150.0, 0.0..=0.4);

            if text_alpha > 0.0 {
                let color = color_from_alpha(ui, text_alpha);

                let text: String = if let Some(formatter) = axis_formatters[axis].as_deref() {
                    formatter(value_main, &axis_range)
                } else {
                    emath::round_to_decimals(value_main, 5).to_string() // hack
                };

                // Custom formatters can return empty string to signal "no label at this resolution"
                if !text.is_empty() {
                    let galley = ui.painter().layout_no_wrap(text, font_id.clone(), color);

                    let mut text_pos = pos_in_gui + vec2(1.0, -galley.size().y);

                    // Make sure we see the labels, even if the axis is off-screen:
                    text_pos[1 - axis] = text_pos[1 - axis]
                        .at_most(transform.frame().max[1 - axis] - galley.size()[1 - axis] - 2.0)
                        .at_least(transform.frame().min[1 - axis] + 1.0);

                    shapes.push(Shape::galley(text_pos, galley));
                }
            }
        }

        fn color_from_alpha(ui: &Ui, alpha: f32) -> Color32 {
            if ui.visuals().dark_mode {
                Rgba::from_white_alpha(alpha).into()
            } else {
                Rgba::from_black_alpha((4.0 * alpha).at_most(1.0)).into()
            }
        }
    }

    fn hover(&self, ui: &Ui, pointer: Pos2, shapes: &mut Vec<Shape>) {
        let Self {
            transform,
            show_x,
            show_y,
            label_formatter,
            items,
            ..
        } = self;

        if !show_x && !show_y {
            return;
        }

        let interact_radius_sq: f32 = (16.0f32).powi(2);

        let candidates = items.iter().filter_map(|item| {
            let item = &**item;
            let closest = item.find_closest(pointer, transform);

            Some(item).zip(closest)
        });

        let closest = candidates
            .min_by_key(|(_, elem)| elem.dist_sq.ord())
            .filter(|(_, elem)| elem.dist_sq <= interact_radius_sq);

        let plot = items::PlotConfig {
            ui,
            transform,
            show_x: *show_x,
            show_y: *show_y,
        };

        if let Some((item, elem)) = closest {
            item.on_hover(elem, shapes, &plot, label_formatter);
        } else {
            let value = transform.value_from_position(pointer);
            items::rulers_at_value(pointer, value, "", &plot, shapes, label_formatter);
        }
    }
}

/// Returns next bigger power in given base
/// e.g.
/// ```ignore
/// use egui::plot::next_power;
/// assert_eq!(next_power(0.01, 10.0), 0.01);
/// assert_eq!(next_power(0.02, 10.0), 0.1);
/// assert_eq!(next_power(0.2,  10.0), 1);
/// ```
fn next_power(value: f64, base: f64) -> f64 {
    assert_ne!(value, 0.0); // can be negative (typical for Y axis)
    base.powi(value.abs().log(base).ceil() as i32)
}

/// Fill in all values between [min, max] which are a multiple of `step_size`
fn generate_marks(step_sizes: [f64; 3], bounds: (f64, f64)) -> Vec<GridMark> {
    let mut steps = vec![];
    fill_marks_between(&mut steps, step_sizes[0], bounds);
    fill_marks_between(&mut steps, step_sizes[1], bounds);
    fill_marks_between(&mut steps, step_sizes[2], bounds);
    steps
}

/// Fill in all values between [min, max] which are a multiple of `step_size`
fn fill_marks_between(out: &mut Vec<GridMark>, step_size: f64, (min, max): (f64, f64)) {
    assert!(max > min);
    let first = (min / step_size).ceil() as i64;
    let last = (max / step_size).ceil() as i64;

    let marks_iter = (first..last).map(|i| {
        let value = (i as f64) * step_size;
        GridMark { value, step_size }
    });
    out.extend(marks_iter);
}
