//! Aurora Lights: chassis RGB control for the Alienware Aurora R16 on Linux and Windows.
//!
//! `aurora-lights`            opens the GUI
//! `aurora-lights --restore`  reapplies the last applied setting (for login autostart)

// No console window on Windows; `--restore` runs silently there.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod autostart;
mod controller;
#[cfg_attr(target_os = "linux", path = "device_linux.rs")]
#[cfg_attr(windows, path = "device_windows.rs")]
mod device;
mod effects;

use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, Sense, Stroke, Vec2};

use controller::{Controller, Rgb};
use effects::{Effect, PALETTE, Settings, Speed};

const PREVIEW_ZONES: usize = 28;
const SWATCH_SIZE: f32 = 30.0;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => run_gui(),
        [flag] if flag == "--restore" => restore(),
        _ => {
            eprintln!("usage: aurora-lights [--restore]");
            ExitCode::from(2)
        }
    }
}

/// Send `settings` to the controller and remember them for `--restore`.
fn apply(settings: &Settings) -> Result<String, String> {
    let mut controller = Controller::open()?;
    controller.play(&settings.sequences(), settings.brightness_for_device())?;
    let mut message = format!(
        "Applied {} to {} zones (platform 0x{:04x}).",
        settings.effect.label(),
        controller.zone_count,
        controller.platform
    );
    if let Err(e) = settings.save() {
        message.push_str(&format!(" Not saved for next login: {e}"));
    }
    Ok(message)
}

fn restore() -> ExitCode {
    let Some(settings) = Settings::load() else {
        return ExitCode::SUCCESS;
    };
    match apply(&settings) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("aurora-lights: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_gui() -> ExitCode {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Aurora Lights")
            .with_inner_size([540.0, 660.0])
            .with_min_inner_size([420.0, 520.0]),
        ..Default::default()
    };
    match eframe::run_native(
        "Aurora Lights",
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("aurora-lights: {e}");
            ExitCode::FAILURE
        }
    }
}

enum Status {
    Idle,
    Busy,
    Done(String),
    Failed(String),
}

struct App {
    settings: Settings,
    /// Which of the two color slots the palette edits.
    editing: usize,
    /// Where the controller was found, if it was.
    device: Option<String>,
    restore_at_login: bool,
    job: Option<Receiver<Result<String, String>>>,
    status: Status,
    started: Instant,
}

impl App {
    fn new() -> Self {
        let status = match autostart::refresh() {
            Ok(()) => Status::Idle,
            Err(e) => Status::Failed(e),
        };
        App {
            settings: Settings::load().unwrap_or_default(),
            editing: 0,
            device: device::find(),
            restore_at_login: autostart::is_enabled(),
            job: None,
            status,
            started: Instant::now(),
        }
    }

    fn start_apply(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let settings = self.settings.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(apply(&settings));
            ctx.request_repaint();
        });
        self.job = Some(rx);
        self.status = Status::Busy;
    }

    fn poll_job(&mut self) {
        let Some(rx) = &self.job else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("apply thread stopped unexpectedly".into()),
        };
        self.job = None;
        self.status = match result {
            Ok(message) => Status::Done(message),
            Err(e) => Status::Failed(e),
        };
        self.device = device::find();
    }

    fn device_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(location) = &self.device {
                ui.colored_label(Color32::from_rgb(0x2E, 0xA0, 0x43), "●");
                ui.label("Controller found").on_hover_text(location);
            } else {
                ui.colored_label(Color32::from_rgb(0xD0, 0x3A, 0x3A), "●");
                ui.label("Controller not found");
            }
            if ui.small_button("Rescan").clicked() {
                self.device = device::find();
            }
        });
    }

    fn effect_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Effect");
        ui.horizontal_wrapped(|ui| {
            for effect in Effect::ALL {
                ui.selectable_value(&mut self.settings.effect, effect, effect.label());
            }
        });
        ui.label(RichText::new(self.settings.effect.description()).weak());
    }

    fn color_section(&mut self, ui: &mut egui::Ui) {
        let count = self.settings.effect.color_count();
        if count == 0 {
            return;
        }
        self.editing = self.editing.min(count - 1);

        section(ui, if count == 1 { "Color" } else { "Colors" });
        if count == 2 {
            ui.horizontal(|ui| {
                for slot in 0..2 {
                    swatch(ui, self.settings.colors[slot], false, 18.0);
                    ui.selectable_value(&mut self.editing, slot, format!("Color {}", slot + 1));
                    ui.add_space(8.0);
                }
            });
            ui.add_space(4.0);
        }

        let current = self.settings.colors[self.editing];
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(6.0);
            for (name, color) in PALETTE {
                if swatch(ui, color, color == current, SWATCH_SIZE)
                    .on_hover_text(name)
                    .clicked()
                {
                    self.settings.colors[self.editing] = color;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Custom:");
            egui::color_picker::color_edit_button_srgb(ui, &mut self.settings.colors[self.editing]);
            let [r, g, b] = self.settings.colors[self.editing];
            ui.label(RichText::new(format!("#{r:02X}{g:02X}{b:02X}")).monospace());
        });
    }

    fn speed_section(&mut self, ui: &mut egui::Ui) {
        if !self.settings.effect.uses_speed() {
            return;
        }
        section(ui, "Speed");
        ui.horizontal(|ui| {
            for speed in Speed::ALL {
                ui.selectable_value(&mut self.settings.speed, speed, speed.label());
            }
        });
    }

    fn brightness_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Brightness");
        ui.add_enabled(
            self.settings.effect != Effect::Off,
            egui::Slider::new(&mut self.settings.brightness, 0..=100).suffix(" %"),
        );
    }

    fn preview_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Preview");
        let sequences = self.settings.sequences();
        let millis = self.started.elapsed().as_millis() as u64;
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::hover());
        let cell = rect.width() / PREVIEW_ZONES as f32;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, Color32::BLACK);
        for zone in 0..PREVIEW_ZONES {
            let color = self.settings.preview_color(&sequences, zone, millis);
            let x = rect.left() + zone as f32 * cell;
            let zone_rect = egui::Rect::from_min_max(
                egui::pos2(x + 1.0, rect.top() + 3.0),
                egui::pos2(x + cell - 1.0, rect.bottom() - 3.0),
            );
            painter.rect_filled(zone_rect, 2.0, rgb(color));
        }
        ui.label(
            RichText::new("Approximate; the controller's zone order may differ.")
                .weak()
                .small(),
        );
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }

    fn apply_row(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let busy = matches!(self.status, Status::Busy);
            let button = egui::Button::new(RichText::new("Apply").strong().size(16.0))
                .min_size(Vec2::new(110.0, 32.0));
            if ui.add_enabled(!busy, button).clicked() {
                self.start_apply(ui.ctx());
            }
            ui.add_space(8.0);
            if ui
                .checkbox(&mut self.restore_at_login, "Restore at login")
                .on_hover_text("Reapply the last applied setting every time you log in.")
                .changed()
                && let Err(e) = autostart::set(self.restore_at_login)
            {
                self.restore_at_login = autostart::is_enabled();
                self.status = Status::Failed(e);
            }
        });
        ui.add_space(6.0);
        match &self.status {
            Status::Idle => {}
            Status::Busy => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Sending to controller…");
                });
            }
            Status::Done(message) => {
                ui.label(message);
            }
            Status::Failed(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_job();
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Aurora Lights");
                self.device_row(ui);
                self.effect_section(ui);
                self.color_section(ui);
                self.speed_section(ui);
                self.brightness_section(ui);
                self.preview_section(ui);
                self.apply_row(ui);
            });
        });
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(12.0);
    ui.label(RichText::new(title).strong().size(15.0));
    ui.add_space(2.0);
}

fn rgb([r, g, b]: Rgb) -> Color32 {
    Color32::from_rgb(r, g, b)
}

fn swatch(ui: &mut egui::Ui, color: Rgb, selected: bool, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let visuals = ui.visuals();
    let stroke = if selected {
        Stroke::new(2.5, visuals.strong_text_color())
    } else if response.hovered() {
        Stroke::new(1.5, visuals.text_color())
    } else {
        visuals.widgets.noninteractive.bg_stroke
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, rgb(color));
    painter.rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Outside);
    response
}
