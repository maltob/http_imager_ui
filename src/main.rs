#![windows_subsystem = "windows"]

mod config;
mod engine;

use crate::config::DeployConfig;
use crate::engine::{DeploymentEngine, DeploymentEvent, is_winpe};
use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use nwg::{MessageChoice, MessageParams, NativeUi};
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use sysinfo::{Disks, Networks};

#[derive(Default, NwgUi)]
pub struct ImagingApp {
    #[nwg_control(size: (700, 390), position: (50, 50), title: "HTTP Imager", flags: "WINDOW|VISIBLE")]
    #[nwg_events(OnWindowClose: [ImagingApp::exit_msg], OnInit: [ImagingApp::setup])]
    window: nwg::Window,

    #[nwg_layout(parent: window, spacing: 1)]
    grid: nwg::GridLayout,

    #[nwg_control(text: "WIM Download URL")]
    #[nwg_layout_item(layout: grid, row: 0, col: 0)]
    download_label: nwg::Label,

    #[nwg_control(text: "", focus: true)]
    #[nwg_layout_item(layout: grid, row: 0, col: 1, col_span: 5)]
    download_url: nwg::TextInput,

    #[nwg_control(flags: "DISABLED")]
    #[nwg_layout_item(layout: grid, col: 0, row: 2, col_span: 6)]
    install_progress: nwg::ProgressBar,

    #[nwg_control(flags: "DISABLED", text: "Ready")]
    #[nwg_layout_item(layout: grid, col: 0, row: 3, col_span: 6)]
    install_status: nwg::Label,

    #[nwg_control(text: "Start Install")]
    #[nwg_layout_item(layout: grid, col: 0, row: 2, row_span: 2, col_span: 6)]
    #[nwg_events(OnButtonClick: [ImagingApp::install_windows])]
    start_button: nwg::Button,

    #[nwg_control(text: "[Advanced] WIM path")]
    #[nwg_layout_item(layout: grid, row: 4, col: 0, col_span: 2)]
    wim_temp_path_label: nwg::Label,

    #[nwg_control(text: "", focus: true)]
    #[nwg_layout_item(layout: grid, row: 4, col: 2, col_span: 4)]
    wim_temp_path: nwg::TextInput,

    #[nwg_control(text: "[Advanced] Temp path")]
    #[nwg_layout_item(layout: grid, row: 5, col: 0, col_span: 2)]
    file_temp_path_label: nwg::Label,

    #[nwg_control(text: "X:\\temp", focus: true)]
    #[nwg_layout_item(layout: grid, row: 5, col: 2, col_span: 4)]
    file_temp_path: nwg::TextInput,

    #[nwg_control(text: "[Advanced] Image index")]
    #[nwg_layout_item(layout: grid, row: 6, col: 0, col_span: 2)]
    image_index_label: nwg::Label,

    #[nwg_control(text: "1", focus: true)]
    #[nwg_layout_item(layout: grid, row: 6, col: 2, col_span: 1)]
    image_index: nwg::TextInput,

    #[nwg_control(text: "Target disk number")]
    #[nwg_layout_item(layout: grid, row: 6, col: 3, col_span: 2)]
    target_disk_label: nwg::Label,

    #[nwg_control(text: "0", focus: true)]
    #[nwg_layout_item(layout: grid, row: 6, col: 5)]
    target_disk: nwg::TextInput,

    #[nwg_control(text: "Status: Unknown")]
    #[nwg_layout_item(layout: grid, row: 7, col: 0, col_span: 6)]
    status_label: nwg::Label,

    #[nwg_control]
    #[nwg_events(OnNotice: [ImagingApp::on_notice])]
    notice: nwg::Notice,

    #[nwg_control(interval: Duration::from_millis(10_000))]
    #[nwg_events(OnNotice: [ImagingApp::update_status_bar])]
    status_timer: nwg::AnimationTimer,

    events: Arc<Mutex<VecDeque<DeploymentEvent>>>,
    running: Arc<Mutex<bool>>,
    auto_reboot: Arc<Mutex<bool>>,
}

impl ImagingApp {
    fn setup(&self) {
        self.download_label.set_visible(true);
        self.install_progress.set_visible(false);
        self.install_status.set_visible(false);

        if !is_winpe() {
            nwg::modal_info_message(
                &self.window,
                "Windows PE required",
                "HTTP Imager can only erase and deploy from Windows PE. The Start button is disabled.",
            );
            self.start_button.set_enabled(false);
        }

        match DeployConfig::load_from_executable() {
            Ok(config) => {
                self.download_url.set_text(&config.download_url);
                self.wim_temp_path
                    .set_text(&config.temp_wim_path.display().to_string());
                self.file_temp_path
                    .set_text(&config.temp_file_path.display().to_string());
                self.image_index.set_text(&config.image_index.to_string());
                self.target_disk
                    .set_text(&config.target_disk.unwrap_or(0).to_string());
                *self.auto_reboot.lock().expect("auto reboot lock poisoned") = config.auto_reboot;
            }
            Err(error) => {
                nwg::modal_info_message(&self.window, "Invalid Settings.ini", &error.to_string());
                self.start_button.set_enabled(false);
            }
        }

        self.update_status_bar();
        self.status_timer.start();
        if self.is_auto_install() && is_winpe() && self.start_button.enabled() {
            self.install_windows();
        }
    }

    fn is_auto_install(&self) -> bool {
        DeployConfig::load_from_executable()
            .map(|config| config.auto_install)
            .unwrap_or(false)
    }

    fn config_from_ui(&self) -> Result<DeployConfig, String> {
        let mut config = DeployConfig::load_from_executable().map_err(|error| error.to_string())?;
        config.download_url = self.download_url.text().trim().to_string();
        if config.download_url.is_empty() {
            return Err("WIM download URL cannot be empty".to_string());
        }
        config.temp_wim_path = std::path::PathBuf::from(self.wim_temp_path.text().trim());
        config.temp_file_path = std::path::PathBuf::from(self.file_temp_path.text().trim());
        config.image_index = self
            .image_index
            .text()
            .trim()
            .parse::<u32>()
            .map_err(|_| "Image index must be a positive integer".to_string())?;
        if config.image_index == 0 {
            return Err("Image index must be greater than zero".to_string());
        }
        config.target_disk = Some(
            self.target_disk
                .text()
                .trim()
                .parse::<u32>()
                .map_err(|_| "Target disk must be a physical disk number".to_string())?,
        );
        Ok(config)
    }

    fn install_windows(&self) {
        if *self.running.lock().expect("running lock poisoned") || !is_winpe() {
            if !is_winpe() {
                nwg::modal_info_message(
                    &self.window,
                    "Windows PE required",
                    "Boot Windows PE before starting an install.",
                );
            }
            return;
        }
        let config = match self.config_from_ui() {
            Ok(config) => config,
            Err(error) => {
                nwg::modal_info_message(&self.window, "Invalid deployment settings", &error);
                return;
            }
        };

        let confirmation = MessageParams {
            buttons: nwg::MessageButtons::YesNo,
            title: "Confirm destructive deployment",
            content: &format!(
                "This will erase physical disk {} and apply index {} from {}. Continue?",
                config.target_disk.unwrap_or(0),
                config.image_index,
                config.download_url
            ),
            icons: nwg::MessageIcons::Warning,
        };
        if !config.auto_install
            && nwg::modal_message(&self.window, &confirmation) != MessageChoice::Yes
        {
            return;
        }

        *self.running.lock().expect("running lock poisoned") = true;
        self.set_ui(true);
        self.install_progress
            .set_range(std::ops::Range { start: 0, end: 100 });
        self.install_progress.set_pos(0);
        self.install_status.set_text("Starting preflight checks");

        let events: Arc<Mutex<VecDeque<DeploymentEvent>>> = Arc::clone(&self.events);
        let running: Arc<Mutex<bool>> = Arc::clone(&self.running);
        let sender = self.notice.sender();
        thread::spawn(move || {
            let mut sink = |event: DeploymentEvent| {
                if let Ok(mut queue) = events.lock() {
                    queue.push_back(event);
                }
                sender.notice();
            };
            let _ = DeploymentEngine::new().run(&config, &mut sink);
            if let Ok(mut state) = running.lock() {
                *state = false;
            }
            sender.notice();
        });
    }

    fn on_notice(&self) {
        while let Some(event) = self
            .events
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop_front())
        {
            match event {
                DeploymentEvent::StageStarted(stage) => self.install_status.set_text(stage.label()),
                DeploymentEvent::StageCompleted(stage) => {
                    self.install_status
                        .set_text(&format!("{} complete", stage.label()));
                    self.install_progress.advance_delta(7);
                }
                DeploymentEvent::DownloadProgress { downloaded, total } => {
                    let text = match total {
                        Some(total) => format!(
                            "Downloading: {} / {} MB",
                            downloaded / 1_048_576,
                            total / 1_048_576
                        ),
                        None => format!("Downloading: {} MB", downloaded / 1_048_576),
                    };
                    self.install_status.set_text(&text);
                }
                DeploymentEvent::Log(message) => self.status_label.set_text(&message),
                DeploymentEvent::Warning(message) => {
                    self.install_status.set_text(&format!("Warning: {message}"))
                }
                DeploymentEvent::Failed { stage, message } => {
                    self.install_status
                        .set_text(&format!("{} failed", stage.label()));
                    nwg::modal_info_message(&self.window, "Deployment failed", &message);
                    self.set_ui(false);
                }
                DeploymentEvent::Completed => {
                    self.install_progress.set_pos(100);
                    self.install_status
                        .set_text("Deployment verified; reboot when ready");
                    self.set_ui(false);
                    if *self.auto_reboot.lock().expect("auto reboot lock poisoned") {
                        let _ = std::process::Command::new("wpeutil").arg("reboot").spawn();
                    }
                }
            }
        }
    }

    fn update_status_bar(&self) {
        let ip = get_first_ip().unwrap_or_else(|| "Undefined".to_string());
        self.status_label
            .set_text(&format!("IP address: {ip}; Disks: {}", get_disk_summary()));
    }

    fn set_ui(&self, running: bool) {
        self.install_progress.set_visible(running);
        self.install_status.set_visible(running);
        self.start_button.set_visible(!running);
        self.download_url.set_enabled(!running);
        self.wim_temp_path.set_enabled(!running);
        self.file_temp_path.set_enabled(!running);
        self.image_index.set_enabled(!running);
        self.target_disk.set_enabled(!running);
    }

    fn exit_msg(&self) {
        if *self.running.lock().expect("running lock poisoned") {
            nwg::modal_info_message(
                &self.window,
                "Deployment in progress",
                "Do not close the window while imaging is running.",
            );
            return;
        }
        nwg::stop_thread_dispatch();
    }
}

fn get_first_ip() -> Option<String> {
    let networks = Networks::new_with_refreshed_list();
    networks
        .iter()
        .flat_map(|(_, data)| data.ip_networks())
        .find_map(|ipnet| {
            let address = ipnet.addr;
            if !address.is_loopback()
                && !address.is_multicast()
                && !address.is_unspecified()
                && !address.to_string().starts_with("169.")
                && !address.to_string().starts_with("fe80:")
            {
                Some(format!("{address}/{}", ipnet.prefix))
            } else {
                None
            }
        })
}

fn get_disk_summary() -> String {
    let disks = Disks::new_with_refreshed_list();
    let disks = disks
        .iter()
        .filter(|disk| !disk.is_removable() && !disk.is_read_only())
        .map(|disk| {
            format!(
                "{} ({} GB)",
                disk.mount_point().display(),
                disk.total_space() / 1_000_000_000
            )
        })
        .collect::<Vec<_>>();
    if disks.is_empty() {
        "none".to_string()
    } else {
        disks.join(", ")
    }
}

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if args.iter().any(|argument| argument == "--silent") {
        std::process::exit(run_silent(&args));
    }
    nwg::init().expect("Failed to initialize the NWG application");
    nwg::Font::set_global_family("Segoe UI").expect("Failed to set Segoe UI font");
    let _app =
        ImagingApp::build_ui(Default::default()).expect("Failed to build the HTTP Imager UI");
    nwg::dispatch_thread_events();
}

fn run_silent(args: &[String]) -> i32 {
    if !is_winpe() {
        write_silent_error("Silent deployment requires Windows PE");
        return 10;
    }
    let config_path = argument_value(args, "--config").unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("Settings.ini")))
            .unwrap_or_else(|| PathBuf::from("Settings.ini"))
    });
    let config = match DeployConfig::load(&config_path) {
        Ok(config) => config,
        Err(error) => {
            write_silent_error(&format!("Invalid configuration: {error}"));
            return 11;
        }
    };
    if !config.auto_install {
        write_silent_error("Silent mode requires [deploy] auto_install=1");
        return 12;
    }

    let mut status = OpenOptions::new()
        .create(true)
        .append(true)
        .open(r"X:\http-imager-silent.log")
        .ok();
    let mut sink = |event: DeploymentEvent| {
        if let Some(file) = status.as_mut() {
            let _ = writeln!(file, "{event:?}");
        }
    };
    match DeploymentEngine::new().run(&config, &mut sink) {
        Ok(()) => {
            if config.auto_reboot {
                match std::process::Command::new("wpeutil.exe")
                    .arg("reboot")
                    .status()
                {
                    Ok(status) if status.success() => return 0,
                    Ok(status) => {
                        write_silent_error(&format!(
                            "Deployment verified, but reboot failed with {:?}",
                            status.code()
                        ));
                        return 13;
                    }
                    Err(error) => {
                        write_silent_error(&format!(
                            "Deployment verified, but reboot failed: {error}"
                        ));
                        return 13;
                    }
                }
            }
            0
        }
        Err(error) => {
            write_silent_error(&error.to_string());
            20
        }
    }
}

fn argument_value(args: &[String], name: &str) -> Option<PathBuf> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| PathBuf::from(&pair[1]))
}

fn write_silent_error(message: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(r"X:\http-imager-silent.log")
    {
        let _ = writeln!(file, "ERROR: {message}");
    }
}
