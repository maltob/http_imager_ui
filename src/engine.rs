use crate::config::DeployConfig;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread::sleep;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Preflight,
    Partition,
    Download,
    VerifyImage,
    ApplyImage,
    Drivers,
    Unattend,
    Staging,
    Winre,
    Bootloader,
    Verify,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Preflight => "Preflight checks",
            Self::Partition => "Partitioning target disk",
            Self::Download => "Downloading Windows image",
            Self::VerifyImage => "Verifying Windows image",
            Self::ApplyImage => "Applying Windows image",
            Self::Drivers => "Installing drivers",
            Self::Unattend => "Applying unattended settings",
            Self::Staging => "Copying staging files",
            Self::Winre => "Configuring Windows Recovery",
            Self::Bootloader => "Installing boot files",
            Self::Verify => "Verifying installation",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeploymentEvent {
    StageStarted(Stage),
    StageCompleted(Stage),
    DownloadProgress { downloaded: u64, total: Option<u64> },
    Log(String),
    Warning(String),
    Failed { stage: Stage, message: String },
    Completed,
}

#[derive(Debug)]
pub struct DeploymentError {
    pub stage: Stage,
    pub message: String,
}

impl DeploymentError {
    fn new(stage: Stage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DeploymentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.stage.label(), self.message)
    }
}

impl std::error::Error for DeploymentError {}

#[derive(Debug)]
pub(crate) struct CommandFailure {
    program: String,
    stderr: String,
}

pub(crate) trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[String]) -> Result<Output, CommandFailure>;
}

pub(crate) struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<Output, CommandFailure> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|error| CommandFailure {
                program: program.to_string(),
                stderr: error.to_string(),
            })?;
        Ok(output)
    }
}

pub struct DeploymentEngine<R = ProcessRunner> {
    runner: R,
}

impl DeploymentEngine<ProcessRunner> {
    pub fn new() -> Self {
        Self {
            runner: ProcessRunner,
        }
    }
}

impl Default for DeploymentEngine<ProcessRunner> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: CommandRunner> DeploymentEngine<R> {
    fn emit<F: FnMut(DeploymentEvent)>(sink: &mut F, event: DeploymentEvent) {
        sink(event);
    }

    fn run_command<F: FnMut(DeploymentEvent)>(
        &self,
        stage: Stage,
        program: &str,
        args: Vec<String>,
        sink: &mut F,
    ) -> Result<Output, DeploymentError> {
        Self::emit(
            sink,
            DeploymentEvent::Log(format!("Running {} {}", program, args.join(" "))),
        );
        let output = self.runner.run(program, &args).map_err(|failure| {
            DeploymentError::new(
                stage,
                format!("{} failed to start: {}", failure.program, failure.stderr),
            )
        })?;
        if output.status.success() {
            Ok(output)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(DeploymentError::new(
                stage,
                format!(
                    "{} exited with code {:?}: {}",
                    program,
                    output.status.code(),
                    stderr
                ),
            ))
        }
    }

    fn run_command_with_success_codes<F: FnMut(DeploymentEvent)>(
        &self,
        stage: Stage,
        program: &str,
        args: Vec<String>,
        success_codes: &[i32],
        sink: &mut F,
    ) -> Result<Output, DeploymentError> {
        Self::emit(
            sink,
            DeploymentEvent::Log(format!("Running {} {}", program, args.join(" "))),
        );
        let output = self.runner.run(program, &args).map_err(|failure| {
            DeploymentError::new(
                stage,
                format!("{} failed to start: {}", failure.program, failure.stderr),
            )
        })?;
        if output.status.success()
            || output
                .status
                .code()
                .is_some_and(|code| success_codes.contains(&code))
        {
            Ok(output)
        } else {
            Err(DeploymentError::new(
                stage,
                format!(
                    "{} exited with code {:?}: {}",
                    program,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ))
        }
    }

    pub fn run<F: FnMut(DeploymentEvent)>(
        &self,
        config: &DeployConfig,
        sink: &mut F,
    ) -> Result<(), DeploymentError> {
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(r"X:\http-imager.log")
            .ok();
        let mut log_sink = |event: DeploymentEvent| {
            if let Some(file) = log.as_mut() {
                let _ = writeln!(file, "{event:?}");
            }
            sink(event);
        };
        self.stage(Stage::Preflight, &mut log_sink, |engine, sink| {
            engine.preflight(config, sink)
        })?;
        self.stage(Stage::Partition, &mut log_sink, |engine, sink| {
            engine.partition(config, sink)
        })?;
        self.stage(Stage::Download, &mut log_sink, |_, sink| {
            download_with_retries(
                &config.download_url,
                &config.temp_wim_path,
                config.expected_size,
                config.expected_sha256.as_deref(),
                sink,
            )
            .map_err(|message| DeploymentError::new(Stage::Download, message))
        })?;
        self.stage(Stage::VerifyImage, &mut log_sink, |engine, sink| {
            engine.verify_wim(config, sink)
        })?;
        self.stage(Stage::ApplyImage, &mut log_sink, |engine, sink| {
            engine.apply_wim(config, sink)
        })?;
        self.optional_stage(
            Stage::Drivers,
            config.driver_download_zip.as_deref(),
            config,
            &mut log_sink,
            |engine, sink| engine.install_drivers(config, sink),
        )?;
        self.optional_stage(
            Stage::Unattend,
            config.unattend_download_path.as_deref(),
            config,
            &mut log_sink,
            |engine, sink| engine.apply_unattend(config, sink),
        )?;
        self.stage(Stage::Staging, &mut log_sink, |engine, sink| {
            engine.stage_files(config, sink)
        })?;
        self.stage(Stage::Winre, &mut log_sink, |engine, sink| {
            engine.configure_winre(sink)
        })?;
        self.stage(Stage::Bootloader, &mut log_sink, |engine, sink| {
            engine.install_bootloader(sink)
        })?;
        self.stage(Stage::Verify, &mut log_sink, |engine, sink| {
            engine.verify_installation(sink)
        })?;
        Self::emit(&mut log_sink, DeploymentEvent::Completed);
        Ok(())
    }

    fn stage<F, T, S>(&self, stage: Stage, sink: &mut S, operation: F) -> Result<T, DeploymentError>
    where
        F: FnOnce(&Self, &mut S) -> Result<T, DeploymentError>,
        S: FnMut(DeploymentEvent),
    {
        Self::emit(sink, DeploymentEvent::StageStarted(stage));
        let result = operation(self, sink);
        match result {
            Ok(value) => {
                Self::emit(sink, DeploymentEvent::StageCompleted(stage));
                Ok(value)
            }
            Err(error) => {
                Self::emit(
                    sink,
                    DeploymentEvent::Failed {
                        stage,
                        message: error.message.clone(),
                    },
                );
                Err(error)
            }
        }
    }

    fn optional_stage<F, T, S>(
        &self,
        stage: Stage,
        configured: Option<&str>,
        config: &DeployConfig,
        sink: &mut S,
        operation: F,
    ) -> Result<(), DeploymentError>
    where
        F: FnOnce(&Self, &mut S) -> Result<T, DeploymentError>,
        S: FnMut(DeploymentEvent),
    {
        if configured.is_none() || configured.is_some_and(str::is_empty) {
            return Ok(());
        }
        if config.continue_on_optional_error {
            Self::emit(sink, DeploymentEvent::StageStarted(stage));
            return match operation(self, sink) {
                Ok(_) => {
                    Self::emit(sink, DeploymentEvent::StageCompleted(stage));
                    Ok(())
                }
                Err(error) => {
                    Self::emit(sink, DeploymentEvent::Warning(error.to_string()));
                    Ok(())
                }
            };
        }
        match self.stage(stage, sink, operation) {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn preflight<F: FnMut(DeploymentEvent)>(
        &self,
        config: &DeployConfig,
        sink: &mut F,
    ) -> Result<(), DeploymentError> {
        if !is_winpe() {
            return Err(DeploymentError::new(
                Stage::Preflight,
                "This program must run in Windows PE",
            ));
        }
        if config.target_disk.is_none() && config.auto_install {
            return Err(DeploymentError::new(
                Stage::Preflight,
                "[target] disk is required when auto_install is enabled",
            ));
        }
        if config.format_script.as_os_str().is_empty() || !config.format_script.exists() {
            return Err(DeploymentError::new(
                Stage::Preflight,
                format!(
                    "Partition script not found: {}",
                    config.format_script.display()
                ),
            ));
        }
        if config.temp_wim_path.as_os_str().is_empty() {
            return Err(DeploymentError::new(
                Stage::Preflight,
                "WIM temporary path is empty",
            ));
        }
        if config.check_url.trim().is_empty() {
            return Err(DeploymentError::new(
                Stage::Preflight,
                "Network check URL is empty",
            ));
        }
        self.run_command(
            Stage::Preflight,
            "wpeutil.exe",
            vec!["WaitForNetwork".to_string()],
            sink,
        )?;
        check_url_with_retries(&config.check_url)
            .map_err(|message| DeploymentError::new(Stage::Preflight, message))?;
        for (name, url) in [
            ("stage ZIP", config.stage_download_zip.as_deref()),
            ("driver ZIP", config.driver_download_zip.as_deref()),
            ("unattend", config.unattend_download_path.as_deref()),
        ] {
            if let Some(url) = url {
                let expanded = expand_machine_vars(url);
                if let Err(message) = check_url_with_retries(&expanded) {
                    if config.continue_on_optional_error {
                        Self::emit(
                            sink,
                            DeploymentEvent::Warning(format!(
                                "{name} preflight skipped: {message}"
                            )),
                        );
                    } else {
                        return Err(DeploymentError::new(Stage::Preflight, message));
                    }
                }
            }
        }
        if let Some(path) = config.stage_folder.as_deref() {
            let path = PathBuf::from(expand_machine_vars(path));
            if !path.exists() {
                return Err(DeploymentError::new(
                    Stage::Preflight,
                    format!("Stage folder does not exist: {}", path.display()),
                ));
            }
        }
        self.inspect_target_disk(config, sink)?;
        Self::emit(
            sink,
            DeploymentEvent::Log(format!("Target disk: {}", config.target_disk.unwrap_or(0))),
        );
        Ok(())
    }

    fn inspect_target_disk<F: FnMut(DeploymentEvent)>(
        &self,
        config: &DeployConfig,
        sink: &mut F,
    ) -> Result<(), DeploymentError> {
        let disk = config.target_disk.unwrap_or(0);
        let probe = PathBuf::from(r"X:\http-imager-disk-probe.txt");
        fs::write(
            &probe,
            format!("select disk {disk}\r\ndetail disk\r\nexit\r\n"),
        )
        .map_err(|error| {
            DeploymentError::new(
                Stage::Preflight,
                format!("Cannot create disk probe: {error}"),
            )
        })?;
        let output = self.run_command(
            Stage::Preflight,
            "diskpart.exe",
            vec!["/s".to_string(), probe.display().to_string()],
            sink,
        );
        let _ = fs::remove_file(&probe);
        let output = output?;
        let detail = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
        if let Some(serial) = config.target_serial.as_deref()
            && !detail.contains(&serial.to_ascii_lowercase())
        {
            return Err(DeploymentError::new(
                Stage::Preflight,
                format!("Target disk {disk} does not match configured serial"),
            ));
        }
        if detail.contains("read-only") && detail.contains("yes") {
            return Err(DeploymentError::new(
                Stage::Preflight,
                format!("Target disk {disk} is read-only"),
            ));
        }
        Ok(())
    }

    fn partition<F: FnMut(DeploymentEvent)>(
        &self,
        config: &DeployConfig,
        sink: &mut F,
    ) -> Result<(), DeploymentError> {
        let disk = config.target_disk.unwrap_or(0);
        let script = fs::read_to_string(&config.format_script).map_err(|error| {
            DeploymentError::new(
                Stage::Partition,
                format!("Cannot read partition script: {error}"),
            )
        })?;
        let selected = replace_disk_selection(&script, disk)
            .map_err(|message| DeploymentError::new(Stage::Partition, message))?;
        let generated = PathBuf::from(r"X:\http-imager-partition.txt");
        fs::write(&generated, selected).map_err(|error| {
            DeploymentError::new(
                Stage::Partition,
                format!("Cannot create generated partition script: {error}"),
            )
        })?;
        let result = self.run_command(
            Stage::Partition,
            "diskpart.exe",
            vec!["/s".to_string(), generated.display().to_string()],
            sink,
        );
        let _ = fs::remove_file(&generated);
        result.map(|_| ())
    }

    fn verify_wim<F: FnMut(DeploymentEvent)>(
        &self,
        config: &DeployConfig,
        sink: &mut F,
    ) -> Result<(), DeploymentError> {
        if !config.temp_wim_path.exists() {
            return Err(DeploymentError::new(
                Stage::VerifyImage,
                format!("WIM was not downloaded: {}", config.temp_wim_path.display()),
            ));
        }
        let output = self.run_command(
            Stage::VerifyImage,
            "dism.exe",
            vec![
                "/Get-WimInfo".to_string(),
                format!("/WimFile:{}", config.temp_wim_path.display()),
                "/English".to_string(),
            ],
            sink,
        )?;
        let text = String::from_utf8_lossy(&output.stdout);
        let expected = format!("Index : {}", config.image_index);
        if !text
            .lines()
            .any(|line| line.trim().eq_ignore_ascii_case(&expected))
        {
            return Err(DeploymentError::new(
                Stage::VerifyImage,
                format!("WIM index {} was not found", config.image_index),
            ));
        }
        Ok(())
    }

    fn apply_wim<F: FnMut(DeploymentEvent)>(
        &self,
        config: &DeployConfig,
        sink: &mut F,
    ) -> Result<(), DeploymentError> {
        self.run_command(
            Stage::ApplyImage,
            "dism.exe",
            vec![
                "/Apply-Image".to_string(),
                format!("/ImageFile:{}", config.temp_wim_path.display()),
                format!("/Index:{}", config.image_index),
                "/ApplyDir:W:\\".to_string(),
                "/CheckIntegrity".to_string(),
                "/Verify".to_string(),
            ],
            sink,
        )?;
        Ok(())
    }

    fn install_drivers<F: FnMut(DeploymentEvent)>(
        &self,
        config: &DeployConfig,
        sink: &mut F,
    ) -> Result<(), DeploymentError> {
        let url = config.driver_download_zip.as_deref().unwrap_or_default();
        let url = expand_machine_vars(url);
        let zip_path = config.temp_file_path.join("drivers.zip");
        download_with_retries(&url, &zip_path, None, None, sink)
            .map_err(|message| DeploymentError::new(Stage::Drivers, message))?;
        let extract_path = config.temp_file_path.join("drivers");
        recreate_directory(&extract_path)
            .map_err(|error| DeploymentError::new(Stage::Drivers, error.to_string()))?;
        extract_zip(&zip_path, &extract_path)
            .map_err(|message| DeploymentError::new(Stage::Drivers, message))?;
        self.run_command(
            Stage::Drivers,
            "dism.exe",
            vec![
                "/Image:W:\\".to_string(),
                "/Add-Driver".to_string(),
                format!("/Driver:{}", extract_path.display()),
                "/Recurse".to_string(),
            ],
            sink,
        )?;
        Ok(())
    }

    fn apply_unattend<F: FnMut(DeploymentEvent)>(
        &self,
        config: &DeployConfig,
        sink: &mut F,
    ) -> Result<(), DeploymentError> {
        let url = expand_machine_vars(config.unattend_download_path.as_deref().unwrap_or_default());
        let target = PathBuf::from(r"W:\Windows\Panther\unattend.xml");
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| DeploymentError::new(Stage::Unattend, error.to_string()))?;
        }
        download_with_retries(&url, &target, None, None, sink)
            .map_err(|message| DeploymentError::new(Stage::Unattend, message))?;
        self.run_command(
            Stage::Unattend,
            "dism.exe",
            vec![
                "/Image:W:\\".to_string(),
                format!("/Apply-Unattend:{}", target.display()),
            ],
            sink,
        )?;
        Ok(())
    }

    fn stage_files<F: FnMut(DeploymentEvent)>(
        &self,
        config: &DeployConfig,
        sink: &mut F,
    ) -> Result<(), DeploymentError> {
        if let Some(path) = config.stage_folder.as_deref() {
            let source = PathBuf::from(expand_machine_vars(path));
            if !source.exists() {
                return Err(DeploymentError::new(
                    Stage::Staging,
                    format!("Stage folder does not exist: {}", source.display()),
                ));
            }
            self.run_command_with_success_codes(
                Stage::Staging,
                "xcopy.exe",
                vec![
                    source.display().to_string(),
                    "W:\\".to_string(),
                    "/E".to_string(),
                    "/H".to_string(),
                    "/Q".to_string(),
                    "/Y".to_string(),
                ],
                &[1],
                sink,
            )?;
        }
        if let Some(url) = config.stage_download_zip.as_deref() {
            let url = expand_machine_vars(url);
            let zip_path = config.temp_file_path.join("stage.zip");
            download_with_retries(&url, &zip_path, None, None, sink)
                .map_err(|message| DeploymentError::new(Stage::Staging, message))?;
            extract_zip(&zip_path, Path::new("W:\\"))
                .map_err(|message| DeploymentError::new(Stage::Staging, message))?;
        }
        Ok(())
    }

    fn configure_winre<F: FnMut(DeploymentEvent)>(
        &self,
        sink: &mut F,
    ) -> Result<(), DeploymentError> {
        let source = Path::new(r"W:\Windows\System32\Recovery\Winre.wim");
        if !source.exists() {
            return Err(DeploymentError::new(
                Stage::Winre,
                "The applied image does not contain Winre.wim",
            ));
        }
        let destination = Path::new(r"R:\Recovery\WindowsRE");
        fs::create_dir_all(destination)
            .map_err(|error| DeploymentError::new(Stage::Winre, error.to_string()))?;
        self.run_command_with_success_codes(
            Stage::Winre,
            "xcopy.exe",
            vec![
                source.display().to_string(),
                destination.display().to_string(),
                "/H".to_string(),
                "/Y".to_string(),
            ],
            &[1],
            sink,
        )?;
        self.run_command(
            Stage::Winre,
            r"W:\Windows\System32\reagentc.exe",
            vec![
                "/Setreimage".to_string(),
                "/Path".to_string(),
                r"R:\Recovery\WindowsRE".to_string(),
                "/Target".to_string(),
                r"W:\Windows".to_string(),
            ],
            sink,
        )?;
        Ok(())
    }

    fn install_bootloader<F: FnMut(DeploymentEvent)>(
        &self,
        sink: &mut F,
    ) -> Result<(), DeploymentError> {
        self.run_command(
            Stage::Bootloader,
            r"W:\Windows\System32\bcdboot.exe",
            vec![
                r"W:\Windows".to_string(),
                "/s".to_string(),
                "S:".to_string(),
                "/f".to_string(),
                "UEFI".to_string(),
            ],
            sink,
        )?;
        Ok(())
    }

    fn verify_installation<F: FnMut(DeploymentEvent)>(
        &self,
        sink: &mut F,
    ) -> Result<(), DeploymentError> {
        for path in [
            Path::new(r"W:\Windows\System32\ntoskrnl.exe"),
            Path::new(r"S:\EFI\Microsoft\Boot\bootmgfw.efi"),
            Path::new(r"R:\Recovery\WindowsRE\Winre.wim"),
        ] {
            if !path.exists() {
                return Err(DeploymentError::new(
                    Stage::Verify,
                    format!("Expected deployment file is missing: {}", path.display()),
                ));
            }
        }
        Self::emit(
            sink,
            DeploymentEvent::Log("Deployment verification passed".to_string()),
        );
        Ok(())
    }
}

fn recreate_directory(path: &Path) -> io::Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    fs::create_dir_all(path)
}

fn replace_disk_selection(script: &str, disk: u32) -> Result<String, String> {
    let mut replaced = false;
    let mut output = String::with_capacity(script.len());
    for line in script.lines() {
        let trimmed = line.trim_start();
        if trimmed.to_ascii_lowercase().starts_with("select disk ") {
            if replaced {
                return Err(
                    "Partition script contains more than one select disk command".to_string(),
                );
            }
            let indent = &line[..line.len() - trimmed.len()];
            output.push_str(&format!("{indent}select disk {disk}\r\n"));
            replaced = true;
        } else {
            output.push_str(line);
            output.push_str("\r\n");
        }
    }
    if !replaced {
        return Err("Partition script must contain a select disk command".to_string());
    }
    Ok(output)
}

fn check_url(url: &str) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .timeout_connect(Some(Duration::from_secs(5)))
        .build()
        .into();
    let response = agent
        .get(url)
        .header("Range", "bytes=0-0")
        .call()
        .map_err(|error| format!("Network check failed for {url}: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Network check returned HTTP {} for {url}",
            response.status()
        ));
    }
    Ok(())
}

fn check_url_with_retries(url: &str) -> Result<(), String> {
    let mut last_error = String::new();
    for attempt in 1..=3 {
        match check_url(url) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = error;
                if attempt < 3 {
                    sleep(Duration::from_secs(2));
                }
            }
        }
    }
    Err(last_error)
}

pub fn download_with_retries<F: FnMut(DeploymentEvent)>(
    url: &str,
    destination: &Path,
    expected_size: Option<u64>,
    expected_sha256: Option<&str>,
    sink: &mut F,
) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
    }
    let partial = destination.with_extension(format!(
        "{}partial",
        destination
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!("{value}."))
            .unwrap_or_default()
    ));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .build()
        .into();
    let mut last_error = String::new();

    for attempt in 1..=3 {
        match download_once(&agent, url, &partial, expected_size, sink) {
            Ok(()) => {
                if let Some(expected) = expected_sha256 {
                    let actual = sha256_file(&partial)
                        .map_err(|error| format!("Cannot hash downloaded file: {error}"))?;
                    if !actual.eq_ignore_ascii_case(expected) {
                        let _ = fs::remove_file(&partial);
                        return Err(format!(
                            "SHA-256 mismatch: expected {expected}, got {actual}"
                        ));
                    }
                }
                fs::rename(&partial, destination)
                    .map_err(|error| format!("Cannot finalize download: {error}"))?;
                return Ok(());
            }
            Err(error) => {
                last_error = error;
                if attempt < 3 {
                    sleep(Duration::from_millis(250 * attempt));
                }
            }
        }
    }
    Err(format!("Download failed after 3 attempts: {last_error}"))
}

fn download_once<F: FnMut(DeploymentEvent)>(
    agent: &ureq::Agent,
    url: &str,
    partial: &Path,
    expected_size: Option<u64>,
    sink: &mut F,
) -> Result<(), String> {
    let mut response = agent
        .get(url)
        .call()
        .map_err(|error| format!("HTTP request failed: {error}"))?;
    let total = response.body().content_length().or(expected_size);
    if let (Some(expected), Some(actual)) = (expected_size, response.body().content_length())
        && expected != actual
    {
        return Err(format!(
            "Content-Length mismatch: expected {expected}, server reported {actual}"
        ));
    }
    let mut file = File::create(partial)
        .map_err(|error| format!("Cannot create {}: {error}", partial.display()))?;
    let mut reader = response.body_mut().as_reader();
    let mut buffer = [0_u8; 1024 * 1024];
    let mut downloaded = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("Download read failed: {error}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|error| format!("Download write failed: {error}"))?;
        downloaded += read as u64;
        sink(DeploymentEvent::DownloadProgress { downloaded, total });
    }
    file.sync_all()
        .map_err(|error| format!("Cannot flush {}: {error}", partial.display()))?;
    if let Some(expected) = expected_size.or(total)
        && downloaded != expected
    {
        return Err(format!(
            "Downloaded {downloaded} bytes, expected {expected}"
        ));
    }
    Ok(())
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_zip(zip_path: &Path, destination: &Path) -> Result<(), String> {
    let file = File::open(zip_path)
        .map_err(|error| format!("Cannot open {}: {error}", zip_path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| format!("Invalid ZIP: {error}"))?;
    archive
        .extract(destination)
        .map_err(|error| format!("ZIP extraction failed: {error}"))
}

fn expand_machine_vars(value: &str) -> String {
    let vendor = sysinfo::Product::vendor_name().unwrap_or_else(|| "NO_VENDOR_FOUND".to_string());
    let model = sysinfo::Product::name().unwrap_or_else(|| "NO_MODEL_FOUND".to_string());
    let sku = sysinfo::Product::stock_keeping_unit().unwrap_or_else(|| "NO_SKU_FOUND".to_string());
    let serial = sysinfo::Product::serial_number().unwrap_or_else(|| "NO_SERIAL_FOUND".to_string());
    value
        .replace("{vendor}", &vendor)
        .replace("{model}", &model)
        .replace("{sku}", &sku)
        .replace("{serial}", &serial)
}

pub fn is_winpe() -> bool {
    Path::new(r"X:\Windows\System32\winpeshl.exe").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_selection_is_replaced_once() {
        let script = "select disk 0\nclean\n";
        assert_eq!(
            replace_disk_selection(script, 4).unwrap(),
            "select disk 4\r\nclean\r\n"
        );
    }

    #[test]
    fn scripts_without_disk_selection_are_rejected() {
        assert!(replace_disk_selection("clean\n", 0).is_err());
    }

    #[test]
    fn scripts_with_multiple_disk_selections_are_rejected() {
        assert!(replace_disk_selection("select disk 0\nselect disk 1\n", 0).is_err());
    }

    #[test]
    fn nonzero_process_exit_is_rejected() {
        let engine = DeploymentEngine::new();
        let mut events = Vec::new();
        let result = engine.run_command(
            Stage::Verify,
            "cmd.exe",
            vec!["/C".to_string(), "exit".to_string(), "7".to_string()],
            &mut |event| events.push(event),
        );
        assert!(result.is_err());
    }

    #[test]
    fn fn_xcopy_style_exit_code_can_be_allowed() {
        let engine = DeploymentEngine::new();
        let mut events = Vec::new();
        let result = engine.run_command_with_success_codes(
            Stage::Staging,
            "cmd.exe",
            vec!["/C".to_string(), "exit".to_string(), "1".to_string()],
            &[1],
            &mut |event| events.push(event),
        );
        assert!(result.is_ok());
    }
}
