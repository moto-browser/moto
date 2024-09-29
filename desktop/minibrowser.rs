/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use directories::ProjectDirs;
use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{
    menu, pos2, CentralPanel, Color32, Frame, Key, Label, Modifiers, PaintCallback, Pos2, RichText, SelectableLabel, TopBottomPanel, Vec2
};
use egui_glow::CallbackFn;
use egui_winit::EventResponse;
use euclid::{Box2D, Length, Point2D, Scale, Size2D};
use gleam::gl;
use glow::NativeFramebuffer;
use log::{trace, warn};
use servo::base::id::WebViewId;
use servo::compositing::windowing::EmbedderEvent;
use servo::config::prefs::PrefValue;
use servo::script_traits::TraversalDirection;
use servo::servo_geometry::DeviceIndependentPixel;
use servo::servo_url::ServoUrl;
use servo::style_traits::DevicePixel;
use servo::webrender_traits::RenderingContext;
use servo::TopLevelBrowsingContextId;
use tinyfiledialogs::open_file_dialog;
use winit::event::{ElementState, MouseButton};

use super::db::{add_bookmark, create_bookmarks_db, get_bookmarks, remove_bookmark};
use super::egui_glue::EguiGlow;
use super::events_loop::EventsLoop;
use super::geometry::winit_position_to_euclid_point;
use super::webview::{LoadStatus, WebViewManager};
use super::window_trait::WindowPortsMethods;
use crate::parser::location_bar_input_to_url;

pub struct Minibrowser {
    pub context: EguiGlow,
    pub event_queue: RefCell<Vec<MinibrowserEvent>>,
    pub toolbar_height: Length<f32, DeviceIndependentPixel>,

    /// The framebuffer object name for the widget surface we should draw to, or None if our widget
    /// surface does not use a framebuffer object.
    widget_surface_fbo: Option<NativeFramebuffer>,

    last_update: Instant,
    last_mouse_position: Option<Point2D<f32, DeviceIndependentPixel>>,
    location: RefCell<String>,

    /// Whether the location has been edited by the user without clicking Go.
    location_dirty: Cell<bool>,

    load_status: LoadStatus,

    status_text: Option<String>,

    show_about_window: Cell<bool>,

    config_dir: String,

    bookmarks: RefCell<Vec<Bookmark>>,
}

pub enum MinibrowserEvent {
    /// Go button clicked.
    Go,
    Back,
    Forward,
    Reload,
    NewWebView,
}

fn truncate_with_ellipsis(input: &str, max_length: usize) -> String {
    if input.chars().count() > max_length {
        let truncated: String = input.chars().take(max_length.saturating_sub(1)).collect();
        format!("{}…", truncated)
    } else {
        input.to_string()
    }
}

pub struct Bookmark {
    pub url: String,
    pub title: String,
}

impl Minibrowser {
    pub fn new(
        rendering_context: &RenderingContext,
        events_loop: &EventsLoop,
        initial_url: ServoUrl,
    ) -> Self {
        let gl = unsafe {
            glow::Context::from_loader_function(|s| rendering_context.get_proc_address(s))
        };

        // Adapted from https://github.com/emilk/egui/blob/9478e50d012c5138551c38cbee16b07bc1fcf283/crates/egui_glow/examples/pure_glow.rs
        #[allow(clippy::arc_with_non_send_sync)]
        let context = EguiGlow::new(events_loop.as_winit(), Arc::new(gl), None);
        let widget_surface_fbo = match rendering_context.context_surface_info() {
            Ok(Some(info)) => NonZeroU32::new(info.framebuffer_object).map(NativeFramebuffer),
            Ok(None) => panic!("Failed to get widget surface info from surfman!"),
            Err(error) => panic!("Failed to get widget surface info from surfman! {error:?}"),
        };

        let config_dir = if let Some(proj_dirs) = ProjectDirs::from("org", "Moto Browser", "Moto") {
            String::from(proj_dirs.config_dir().to_str().unwrap_or(""))
            // Linux:   /home/alice/.config/barapp
            // Windows: C:\Users\Alice\AppData\Roaming\Foo Corp\Bar App
            // macOS:   /Users/Alice/Library/Application Support/com.Foo-Corp.Bar-App
        } else {
            // TODO: Handle this case somehow
            String::from("")
        };

        if !config_dir.is_empty() {
            // Set up user config things (bookmarks)

            // Check if config dir exists
            let path = Path::new(&config_dir);
            if !path.exists() {
                fs::create_dir_all(path).unwrap();
            }
            // Check if bookmarks.sqlite exists, if not then create it
            let path = Path::new(&config_dir).join("bookmarks.sqlite");
            if !path.exists() {
                create_bookmarks_db(config_dir.as_str()).expect("Failed to create bookmarks DB!");
            }
        }

        let bookmarks = get_bookmarks(&config_dir);

        Self {
            context,
            event_queue: RefCell::new(vec![]),
            toolbar_height: Default::default(),
            widget_surface_fbo,
            last_update: Instant::now(),
            last_mouse_position: None,
            location: RefCell::new(initial_url.to_string()),
            location_dirty: false.into(),
            load_status: LoadStatus::LoadComplete,
            status_text: None,
            show_about_window: false.into(),
            config_dir,
            bookmarks: RefCell::new(bookmarks),
        }
    }

    /// Preprocess the given [winit::event::WindowEvent], returning unconsumed for mouse events in
    /// the Servo browser rect. This is needed because the CentralPanel we create for our webview
    /// would otherwise make egui report events in that area as consumed.
    pub fn on_window_event(
        &mut self,
        window: &winit::window::Window,
        event: &winit::event::WindowEvent,
    ) -> EventResponse {
        let mut result = self.context.on_window_event(window, event);
        result.consumed &= match event {
            winit::event::WindowEvent::CursorMoved { position, .. } => {
                let scale = Scale::<_, DeviceIndependentPixel, _>::new(
                    self.context.egui_ctx.pixels_per_point(),
                );
                self.last_mouse_position =
                    Some(winit_position_to_euclid_point(*position).to_f32() / scale);
                self.last_mouse_position
                    .map_or(false, |p| self.is_in_browser_rect(p))
            },
            winit::event::WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Forward,
                ..
            } => {
                self.event_queue
                    .borrow_mut()
                    .push(MinibrowserEvent::Forward);
                true
            },
            winit::event::WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Back,
                ..
            } => {
                self.event_queue.borrow_mut().push(MinibrowserEvent::Back);
                true
            },
            winit::event::WindowEvent::MouseWheel { .. }
            | winit::event::WindowEvent::MouseInput { .. } => self
                .last_mouse_position
                .map_or(false, |p| self.is_in_browser_rect(p)),
            _ => true,
        };
        result
    }

    /// Return true iff the given position is in the Servo browser rect.
    fn is_in_browser_rect(&self, position: Point2D<f32, DeviceIndependentPixel>) -> bool {
        position.y < self.toolbar_height.get()
    }

    /// Create a frameless button with square sizing, as used in the toolbar.
    fn toolbar_button(text: &str) -> egui::Button {
        egui::Button::new(text)
            .frame(false)
            .min_size(Vec2 { x: 20.0, y: 20.0 })
    }

    /// Draws a browser tab, checking for clicks and returns an appropriate [EmbedderEvent]
    /// Using a custom widget here would've been nice, but it doesn't seem as though egui
    /// supports that, so we arrange multiple Widgets in a way that they look connected.
    fn browser_tab(
        ui: &mut egui::Ui,
        label: &str,
        selected: bool,
        webview_id: TopLevelBrowsingContextId,
    ) -> Option<EmbedderEvent> {
        let old_item_spacing = ui.spacing().item_spacing;
        let old_visuals = ui.visuals().clone();
        let active_bg_color = old_visuals.widgets.active.weak_bg_fill;
        let inactive_bg_color = old_visuals.window_fill;
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

        let visuals = ui.visuals_mut();
        // Remove the stroke so we don't see the border between the close button and the label
        visuals.widgets.active.bg_stroke.width = 0.0;
        visuals.widgets.hovered.bg_stroke.width = 0.0;
        // Now we make sure the fill color is always the same, irrespective of state, that way
        // we can make sure that both the label and close button have the same background color
        visuals.widgets.noninteractive.weak_bg_fill = inactive_bg_color;
        visuals.widgets.inactive.weak_bg_fill = inactive_bg_color;
        visuals.widgets.hovered.weak_bg_fill = active_bg_color;
        visuals.widgets.active.weak_bg_fill = active_bg_color;
        visuals.selection.bg_fill = active_bg_color;
        visuals.selection.stroke.color = visuals.widgets.active.fg_stroke.color;
        visuals.widgets.hovered.fg_stroke.color = visuals.widgets.active.fg_stroke.color;

        // Expansion would also show that they are 2 separate widgets
        visuals.widgets.active.expansion = 0.0;
        visuals.widgets.hovered.expansion = 0.0;
        // The rounding is changed so it looks as though the 2 widgets are a single widget
        // with a uniform rounding
        let rounding = egui::Rounding {
            ne: 0.0,
            nw: 4.0,
            sw: 4.0,
            se: 0.0,
        };
        visuals.widgets.active.rounding = rounding;
        visuals.widgets.hovered.rounding = rounding;
        visuals.widgets.inactive.rounding = rounding;

        let tab = ui.add(SelectableLabel::new(
            selected,
            truncate_with_ellipsis(label, 20),
        ));
        let tab = tab.on_hover_ui(|ui| {
            ui.label(label);
        });

        let rounding = egui::Rounding {
            ne: 4.0,
            nw: 0.0,
            sw: 0.0,
            se: 4.0,
        };
        let visuals = ui.visuals_mut();
        visuals.widgets.active.rounding = rounding;
        visuals.widgets.hovered.rounding = rounding;
        visuals.widgets.inactive.rounding = rounding;

        let fill_color = if selected || tab.hovered() {
            active_bg_color
        } else {
            inactive_bg_color
        };

        ui.spacing_mut().item_spacing = old_item_spacing;
        let close_button = ui.add(egui::Button::new("X").fill(fill_color));
        *ui.visuals_mut() = old_visuals;
        if close_button.clicked() || close_button.middle_clicked() || tab.middle_clicked() {
            Some(EmbedderEvent::CloseWebView(webview_id))
        } else if !selected && tab.clicked() {
            Some(EmbedderEvent::FocusWebView(webview_id))
        } else {
            None
        }
    }

    /// Update the minibrowser, but don’t paint.
    /// If `servo_framebuffer_id` is given, set up a paint callback to blit its contents to our
    /// CentralPanel when [`Minibrowser::paint`] is called.
    pub fn update(
        &mut self,
        window: &winit::window::Window,
        webviews: &mut WebViewManager<dyn WindowPortsMethods>,
        servo_framebuffer_id: Option<gl::GLuint>,
        reason: &'static str,
    ) {
        let now = Instant::now();
        trace!(
            "{:?} since last update ({})",
            now - self.last_update,
            reason
        );
        let Self {
            context,
            event_queue,
            toolbar_height,
            widget_surface_fbo,
            last_update,
            location,
            location_dirty,
            ..
        } = self;
        let widget_fbo = *widget_surface_fbo;
        let _duration = context.run(window, |ctx| {
            // TODO: While in fullscreen add some way to mitigate the increased phishing risk
            // when not displaying the URL bar: https://github.com/servo/servo/issues/32443
            let is_config = webviews.current_url_string().map_or(false, |url| {
                url == "moto:config"
            });
            if window.fullscreen().is_none() {
                let frame = egui::Frame::default()
                    .fill(ctx.style().visuals.window_fill)
                    .inner_margin(4.0);
                TopBottomPanel::top("toolbar").frame(frame).show(ctx, |ui| {
                    menu::bar(ui, |ui| {
                        ui.menu_button("File", |ui| {
                            if ui.button("Open File").clicked() {
                                let mut file_url = String::from("file:///");
                                let file_path = open_file_dialog("File Picker", "C:/", None);
                                if let Some(path) = file_path {
                                    file_url.push_str(&path);
                                    *location.borrow_mut() = file_url;
                                    event_queue.borrow_mut().push(MinibrowserEvent::Go);
                                }
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Exit").clicked() {
                                std::process::exit(0);
                            }
                        });
                        ui.menu_button("Bookmarks", |ui| {
                            let bookmarks = self.bookmarks.borrow();
                            for bookmark in bookmarks.as_slice() {
                                let button = egui::Button::new(bookmark.title.as_str())
                                    .min_size((256.0, 20.0).into());
                                if ui.add(button).clicked() {
                                    *location.borrow_mut() = bookmark.url.clone();
                                    event_queue.borrow_mut().push(MinibrowserEvent::Go);
                                    ui.close_menu();
                                }
                            }
                        });
                        ui.menu_button("History", |ui| {
                            let history = webviews.history();
                            for url in history {
                                // TODO: Prevent Servo from receiving cursor events while hovering these
                                let button =
                                    egui::Button::new(url.as_str()).min_size((256.0, 20.0).into());
                                if ui.add(button).clicked() {
                                    *location.borrow_mut() = url.to_string();
                                    event_queue.borrow_mut().push(MinibrowserEvent::Go);
                                    ui.close_menu();
                                }
                            }
                        });
                        ui.menu_button("Help", |ui| {
                            if ui.button("About Moto").clicked() {
                                self.show_about_window.set(true);
                                ui.close_menu();
                            }
                        });
                    });
                    ui.allocate_ui_with_layout(
                        ui.available_size(),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            if ui.add(Minibrowser::toolbar_button("⏴")).clicked() {
                                event_queue.borrow_mut().push(MinibrowserEvent::Back);
                            }
                            if ui.add(Minibrowser::toolbar_button("⏵")).clicked() {
                                event_queue.borrow_mut().push(MinibrowserEvent::Forward);
                            }

                            match self.load_status {
                                LoadStatus::LoadStart | LoadStatus::HeadParsed => {
                                    if ui.add(Minibrowser::toolbar_button("X")).clicked() {
                                        warn!("Do not support stop yet.");
                                    }
                                },
                                LoadStatus::LoadComplete => {
                                    if ui.add(Minibrowser::toolbar_button("↻")).clicked() {
                                        event_queue.borrow_mut().push(MinibrowserEvent::Reload);
                                    }
                                },
                            }
                            ui.add_space(2.0);

                            ui.allocate_ui_with_layout(
                                ui.available_size(),
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let has_bookmark = self
                                        .bookmarks
                                        .borrow()
                                        .iter()
                                        .any(|b| b.url == *location.borrow());
                                    let star = if has_bookmark { "★" } else { "☆" };
                                    if ui.add(Minibrowser::toolbar_button(star)).clicked() {
                                        let Some(webview) = webviews.focused_webview() else {
                                            return;
                                        };
                                        let url = webview.url.clone().unwrap().to_string();
                                        let title = webview.title.clone().unwrap_or("".into());
                                        if !has_bookmark {
                                            // Add bookmark to DB
                                            if add_bookmark(&self.config_dir, &url, &title).is_ok()
                                            {
                                                self.bookmarks
                                                    .borrow_mut()
                                                    .push(Bookmark { url, title });
                                            }
                                        } else {
                                            // Remove bookmark from DB
                                            if remove_bookmark(&self.config_dir, &url).is_ok() {
                                                self.bookmarks
                                                    .borrow_mut()
                                                    .retain(|b| b.url != url);
                                            }
                                        }
                                    }
                                    let location_id = egui::Id::new("location_input");
                                    let location_field = ui.add_sized(
                                        ui.available_size(),
                                        egui::TextEdit::singleline(&mut *location.borrow_mut())
                                            .id(location_id),
                                    );

                                    if location_field.changed() {
                                        location_dirty.set(true);
                                    }
                                    if ui.input(|i| {
                                        i.clone().consume_key(Modifiers::COMMAND, Key::L)
                                    }) {
                                        location_field.request_focus();
                                        if let Some(mut state) =
                                            TextEditState::load(ui.ctx(), location_id)
                                        {
                                            // Select the whole input.
                                            state.cursor.set_char_range(Some(CCursorRange::two(
                                                CCursor::new(0),
                                                CCursor::new(location.borrow().len()),
                                            )));
                                            state.store(ui.ctx(), location_id);
                                        }
                                    }
                                    if location_field.lost_focus()
                                        && ui.input(|i| i.clone().key_pressed(Key::Enter))
                                    {
                                        event_queue.borrow_mut().push(MinibrowserEvent::Go);
                                        location_dirty.set(false);
                                    }
                                },
                            );
                        },
                    );
                });

                // About Window
                if self.show_about_window.get() {
                    let size = window.inner_size();
                    egui::Window::new("About")
                        .min_size((256.0, 256.0))
                        .collapsible(false)
                        .pivot(egui::Align2::CENTER_CENTER)
                        .default_pos((size.width as f32 / 2.0, size.height as f32 / 2.0))
                        .title_bar(true)
                        .show(ctx, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.add(egui::Image::new(egui::include_image!(
                                    "../resources/moto_1024.png"
                                )));
                                let text = egui::RichText::new(crate::moto_version()).size(16.0);
                                ui.label(text);
                                ui.add_space(8.0);
                                if ui.button("Close").clicked() {
                                    self.show_about_window.set(false);
                                }
                            });
                        });
                }
            };

            let mut embedder_events = vec![];

            // A simple Tab header strip
            TopBottomPanel::top("tabs").show(ctx, |ui| {
                ui.allocate_ui_with_layout(
                    ui.available_size(),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        for (webview_id, webview) in webviews.webviews().into_iter() {
                            let label = match (&webview.title, &webview.url) {
                                (Some(title), _) if !title.is_empty() => title,
                                (_, Some(url)) => &url.to_string(),
                                _ => "New Tab",
                            };
                            if let Some(event) =
                                Self::browser_tab(ui, label, webview.focused, webview_id)
                            {
                                location_dirty.set(false);
                                embedder_events.push(event);
                            }
                        }
                        if ui.add(Minibrowser::toolbar_button("+")).clicked() {
                            event_queue.borrow_mut().push(MinibrowserEvent::NewWebView);
                        }
                    },
                );
            });

            // The toolbar height is where the Context’s available rect starts.
            // For reasons that are unclear, the TopBottomPanel’s ui cursor exceeds this by one egui
            // point, but the Context is correct and the TopBottomPanel is wrong.
            *toolbar_height = Length::new(ctx.available_rect().min.y);

            let scale =
                Scale::<_, DeviceIndependentPixel, DevicePixel>::new(ctx.pixels_per_point());
            let Some(focused_webview_id) = webviews.focused_webview_id() else {
                return;
            };
            let Some(webview) = webviews.get_mut(focused_webview_id) else {
                return;
            };

            if is_config {
                // Paint the config page
                CentralPanel::default()
                    .frame(Frame::none().fill(Color32::WHITE))
                    .show(ctx, |ui| {
                        let prefs = servo::config::prefs::pref_map();
                        let mut sorted_prefs = prefs.iter().collect::<Vec<_>>();
                        sorted_prefs.sort_by_key(|k| k.0.clone());
                        let mut prefs_to_set = HashMap::new();
                        egui::containers::ScrollArea::vertical().show(ui, |ui| {
                            for (k, v) in sorted_prefs {
                                ui.columns(2, |cols| {
                                    cols[0].vertical(|ui| {
                                        ui.label(
                                            RichText::new(k.clone())
                                                .color(Color32::BLACK)
                                                .size(16.0),
                                        );
                                    });
                                    cols[1].vertical(|ui| {
                                        match v {
                                            PrefValue::Float(f) => {
                                                let mut num_text = f.to_string();
                                                if ui.text_edit_singleline(&mut num_text).changed()
                                                {
                                                    let value = num_text.parse();
                                                    if let Ok(v) = value {
                                                        prefs_to_set.insert(
                                                            k.to_owned(),
                                                            PrefValue::Float(v),
                                                        );
                                                    }
                                                }
                                            },
                                            PrefValue::Int(i) => {
                                                let mut num_text = i.to_string();
                                                if ui.text_edit_singleline(&mut num_text).changed()
                                                {
                                                    let value = num_text.parse();
                                                    if let Ok(v) = value {
                                                        prefs_to_set.insert(
                                                            k.to_owned(),
                                                            PrefValue::Int(v),
                                                        );
                                                    }
                                                }
                                            },
                                            PrefValue::Str(mut s) => {
                                                if ui.text_edit_singleline(&mut s).changed() {
                                                    prefs_to_set
                                                        .insert(k.to_owned(), PrefValue::Str(s));
                                                }
                                            },
                                            PrefValue::Bool(mut b) => {
                                                if ui.checkbox(&mut b, "").clicked() {
                                                    prefs_to_set
                                                        .insert(k.to_owned(), PrefValue::Bool(b));
                                                }
                                            },
                                            PrefValue::Array(_) => {
                                                // TODO: Support this
                                                // There's only a single pref that takes this right now,
                                                // shell.background-color.rgba
                                            },
                                            PrefValue::Missing => {},
                                        }
                                    });
                                });
                            }
                        });
                        prefs_to_set.iter().for_each(|(k, v)| {
                            if let Err(e) = prefs.set(k, v.clone()) {
                                warn!("Failed to set pref: {}", e);
                            }
                        });
                    });
            } else {
                // Paint Servo
                CentralPanel::default()
                    .frame(Frame::none())
                    .show(ctx, |ui| {
                        let Pos2 { x, y } = ui.cursor().min;
                        let Vec2 {
                            x: width,
                            y: height,
                        } = ui.available_size();
                        let rect = Box2D::from_origin_and_size(
                            Point2D::new(x, y),
                            Size2D::new(width, height),
                        ) * scale;
                        if rect != webview.rect {
                            webview.rect = rect;
                            embedder_events
                                .push(EmbedderEvent::MoveResizeWebView(focused_webview_id, rect));
                        }
                        let min = ui.cursor().min;
                        let size = ui.available_size();
                        let rect = egui::Rect::from_min_size(min, size);
                        ui.allocate_space(size);

                        let Some(servo_fbo) = servo_framebuffer_id else {
                            return;
                        };

                        if let Some(status_text) = &self.status_text {
                            egui::containers::popup::show_tooltip_at(
                                ctx,
                                ui.layer_id(),
                                "tooltip layer".into(),
                                pos2(0.0, ctx.available_rect().max.y),
                                |ui| ui.add(Label::new(status_text.clone()).extend()),
                            );
                        }

                        ui.painter().add(PaintCallback {
                            rect,
                            callback: Arc::new(CallbackFn::new(move |info, painter| {
                                use glow::HasContext as _;
                                let clip = info.viewport_in_pixels();
                                let x = clip.left_px as gl::GLint;
                                let y = clip.from_bottom_px as gl::GLint;
                                let width = clip.width_px as gl::GLsizei;
                                let height = clip.height_px as gl::GLsizei;
                                unsafe {
                                    painter.gl().clear_color(0.0, 0.0, 0.0, 0.0);
                                    painter.gl().scissor(x, y, width, height);
                                    painter.gl().enable(gl::SCISSOR_TEST);
                                    painter.gl().clear(gl::COLOR_BUFFER_BIT);
                                    painter.gl().disable(gl::SCISSOR_TEST);

                                    let servo_fbo =
                                        NonZeroU32::new(servo_fbo).map(NativeFramebuffer);
                                    painter
                                        .gl()
                                        .bind_framebuffer(gl::READ_FRAMEBUFFER, servo_fbo);
                                    painter
                                        .gl()
                                        .bind_framebuffer(gl::DRAW_FRAMEBUFFER, widget_fbo);
                                    painter.gl().blit_framebuffer(
                                        x,
                                        y,
                                        x + width,
                                        y + height,
                                        x,
                                        y,
                                        x + width,
                                        y + height,
                                        gl::COLOR_BUFFER_BIT,
                                        gl::NEAREST,
                                    );
                                    painter.gl().bind_framebuffer(gl::FRAMEBUFFER, widget_fbo);
                                }
                            })),
                        });
                    });
            }

            if !embedder_events.is_empty() {
                webviews.handle_window_events(embedder_events);
            }

            *last_update = now;
        });
    }

    /// Paint the minibrowser, as of the last update.
    pub fn paint(&mut self, window: &winit::window::Window) {
        unsafe {
            use glow::HasContext as _;
            self.context
                .painter
                .gl()
                .bind_framebuffer(gl::FRAMEBUFFER, self.widget_surface_fbo);
        }
        self.context.paint(window);
    }

    /// Takes any outstanding events from the [Minibrowser], converting them to [EmbedderEvent] and
    /// routing those to the App event queue.
    pub fn queue_embedder_events_for_minibrowser_events(
        &self,
        browser: &WebViewManager<dyn WindowPortsMethods>,
        app_event_queue: &mut Vec<EmbedderEvent>,
    ) {
        for event in self.event_queue.borrow_mut().drain(..) {
            let browser_id = browser.focused_webview_id().unwrap();
            match event {
                MinibrowserEvent::Go => {
                    let location = self.location.borrow();
                    if let Some(url) = location_bar_input_to_url(&location.clone()) {
                        app_event_queue.push(EmbedderEvent::LoadUrl(browser_id, url));
                    } else {
                        warn!("failed to parse location");
                        break;
                    }
                },
                MinibrowserEvent::Back => {
                    app_event_queue.push(EmbedderEvent::Navigation(
                        browser_id,
                        TraversalDirection::Back(1),
                    ));
                },
                MinibrowserEvent::Forward => {
                    app_event_queue.push(EmbedderEvent::Navigation(
                        browser_id,
                        TraversalDirection::Forward(1),
                    ));
                },
                MinibrowserEvent::Reload => {
                    let browser_id = browser.focused_webview_id().unwrap();
                    app_event_queue.push(EmbedderEvent::Reload(browser_id));
                },
                MinibrowserEvent::NewWebView => {
                    self.location_dirty.set(false);
                    let url = ServoUrl::parse("moto:newtab").unwrap();
                    app_event_queue.push(EmbedderEvent::NewWebView(url, WebViewId::new()));
                },
            }
        }
    }

    /// Updates the location field from the given [WebViewManager], unless the user has started
    /// editing it without clicking Go, returning true iff it has changed (needing an egui update).
    pub fn update_location_in_toolbar(
        &mut self,
        browser: &mut WebViewManager<dyn WindowPortsMethods>,
    ) -> bool {
        // User edited without clicking Go?
        if self.location_dirty.get() {
            return false;
        }

        match browser.current_url_string() {
            Some(location) if location != *self.location.get_mut() => {
                self.location = RefCell::new(location.to_owned());
                true
            },
            _ => false,
        }
    }

    /// Updates the spinner from the given [WebViewManager], returning true iff it has changed
    /// (needing an egui update).
    pub fn update_spinner_in_toolbar(
        &mut self,
        browser: &mut WebViewManager<dyn WindowPortsMethods>,
    ) -> bool {
        let need_update = browser.load_status() != self.load_status;
        self.load_status = browser.load_status();
        need_update
    }

    pub fn update_status_text(
        &mut self,
        browser: &mut WebViewManager<dyn WindowPortsMethods>,
    ) -> bool {
        let need_update = browser.status_text() != self.status_text;
        self.status_text = browser.status_text();
        need_update
    }

    /// Updates all fields taken from the given [WebViewManager], such as the location field.
    /// Returns true iff the egui needs an update.
    pub fn update_webview_data(
        &mut self,
        browser: &mut WebViewManager<dyn WindowPortsMethods>,
    ) -> bool {
        // Note: We must use the "bitwise OR" (|) operator here instead of "logical OR" (||)
        //       because logical OR would short-circuit if any of the functions return true.
        //       We want to ensure that all functions are called. The "bitwise OR" operator
        //       does not short-circuit.
        self.update_location_in_toolbar(browser)
            | self.update_spinner_in_toolbar(browser)
            | self.update_status_text(browser)
    }
}
