
#![windows_subsystem = "windows"]

extern crate native_windows_gui as nwg;
extern crate native_windows_derive as nwd;

use std::{  fs::{self, File}, io::Error, ops::Range, path::Path, process::{ExitStatus, Output}, sync::{Arc, RwLock}, thread::sleep, time::{Duration, SystemTime}};

use configparser::{ini::Ini};
use nwd::NwgUi;
use nwg::{ ImageData, MessageChoice, MessageParams, NativeUi};

use std::thread;
use std::process::Command;
use sysinfo::{Disks, Networks, Product};

#[derive(Default, NwgUi)]
pub struct ImagingApp {
    #[nwg_control(size: (600, 300), position: (50, 50), title: "Simple Installer", flags: "WINDOW|VISIBLE")]
    #[nwg_events( OnWindowClose: [ImagingApp::exit_msg] , OnInit: [ImagingApp::setup])]
    window: nwg::Window,

    #[nwg_layout(parent: window, spacing: 1)]
    grid: nwg::GridLayout,

    #[nwg_control(text: "WIM Download URL")]
    #[nwg_layout_item(layout: grid, row: 0, col: 0)]
    download_label: nwg::Label,

    #[nwg_control(text: "", focus: true)]
    #[nwg_layout_item(layout: grid, row: 0, col: 1, col_span: 4)]
    download_url: nwg::TextInput,
   
    #[nwg_control(flags:"DISABLED")]
    #[nwg_layout_item(layout: grid, col: 0, row: 2, col_span: 5)]
    install_progress: nwg::ProgressBar,
    #[nwg_control(flags:"DISABLED", text:"Started")]
    #[nwg_layout_item(layout: grid, col: 0, row: 3, col_span: 5)]
    install_status: nwg::Label,


    #[nwg_control(text: "Start Install")]
    #[nwg_layout_item(layout: grid, col: 0, row: 2, row_span: 2, col_span: 5)]
    #[nwg_events( OnButtonClick: [ImagingApp::install_windows] )]
    start_button: nwg::Button,


    #[nwg_control(text: "[Adv] Temp WIM Path")]
    #[nwg_layout_item(layout: grid, row: 4, col: 0, col_span:2)]
    iso_temp_path_label: nwg::Label,

    #[nwg_control(text: "", focus: true)]
    #[nwg_layout_item(layout: grid, row: 4, col: 2, col_span: 3)]
    wim_temp_path: nwg::TextInput,

    #[nwg_control(text: "[Adv] Temp File Path")]
    #[nwg_layout_item(layout: grid, row: 5, col: 0, col_span:2)]
    file_temp_path_label: nwg::Label,

    #[nwg_control(text: "X:\\temp", focus: true)]
    #[nwg_layout_item(layout: grid, row: 5, col: 2, col_span: 3)]
    file_temp_path: nwg::TextInput,

    #[nwg_control(text: "[Adv] Image Index")]
    #[nwg_layout_item(layout: grid, row: 6, col: 0, col_span:2)]
    image_index_label: nwg::Label,

    #[nwg_control(text: "1", focus: true)]
    #[nwg_layout_item(layout: grid, row: 6, col: 2, col_span: 3)]
    image_index: nwg::TextInput,

    #[nwg_control(text: "Status: Unknown")]
    #[nwg_layout_item(layout: grid, row: 7, col: 0, col_span:5)]
    status_label: nwg::Label,

    #[nwg_control]
    #[nwg_events( OnNotice: [ImagingApp::on_notice] )]
    notice: nwg::Notice,

    #[nwg_control(interval: Duration::from_millis(10_000))]
    #[nwg_events( OnNotice: [ImagingApp::update_status_bar] )]
    status_timer: nwg::AnimationTimer,

    compute: Arc<RwLock<String>>,

}


impl ImagingApp {
    fn setup (&self) {
        
        if !ImagingApp::is_pe() {
            nwg::modal_info_message(&self.window, "Error", "This program should only be run from Windows PE! \r\n Disabling any disk wipe capabilities.");
            //nwg::stop_thread_dispatch();
        }

        

        let mut config = Ini::new();
        // If we are missing a settings file --- don't try to load it.
        if Path::new("Settings.ini").exists() {
            _  = config.load("Settings.ini");
        }

        // Get the download url we want to use
        let dl_url  = config.get("os","download_url").unwrap_or("https://server.tld/Win11.wim".to_string());
        self.download_url.set_text(dl_url.as_str());

        // Default to the W: drive for install based on the default diskpart
        let temp_wim_path  = config.get("storage","temp_wim_path").unwrap_or("W:\\install.wim".to_string());
        self.wim_temp_path.set_text(temp_wim_path.as_str());

        // Default to the W: drive for install based on the default diskpart
        let temp_file_path  = config.get("storage","temp_file_path").unwrap_or("X:\\temp".to_string());
        self.file_temp_path.set_text(temp_file_path.as_str());

        // Default to index 1 for the WIM
        let image_index_config = config.get("os","index").unwrap_or("1".to_string());
        self.image_index.set_text(image_index_config.as_str());

        // Look for a network check URL
        let network_check_url = config.get("network","check_url").unwrap_or(dl_url.to_string());
        
        self.update_status_bar();
        self.status_timer.start();
        if ImagingApp::is_autoinstall() {
            match ImagingApp::wait_for_network(30, &network_check_url) {
                Ok(true) => self.install_windows(),
                _ => {
                    nwg::modal_info_message(&self.window, "Error installing", "Failed to verify network connectivity for install. Please ensure network connection is correct");
                    ()
                }
            };
            
        }
    }


    fn install_windows(&self) {
        self.set_ui(true);

        self.install_progress.set_range( Range {start: 0, end: 100} );
        self.install_progress.advance_delta(15);

        if !ImagingApp::check_url_valid(self.download_url.text()).is_ok_and(|v| v) {
            nwg::modal_info_message(&self.window, "Invalid URL", &format!("The URL {} is not valid", self.download_url.text()));
            self.set_ui(false);
            return
        }

        let wim_path = match self.wim_temp_path.text().as_str() {
                        "" => ImagingApp::locate_wim_save_space().expect("Failed to locate a save location for the wim"),
                        _ => self.wim_temp_path.text()
        };

        let wim_image_index = match self.image_index.text().parse::<u8>()  {
                        Ok(x) => x,
                        Err(_) => 1
        };

        if !ImagingApp::is_autoinstall() {
            let params =  MessageParams {buttons: nwg::MessageButtons::YesNo, title: "Confirm wipe disk and install?", 
                    content:"Confirm wiping disk 0 and installing windows", icons: nwg::MessageIcons::Question};
            let confirm_wipe = nwg::modal_message(&self.window, &params);
            let _ = match confirm_wipe {
                MessageChoice::Yes => (),
                _ => {self.set_ui(false);return;}
            };
        }


        let sender = self.notice.sender();
        let url = self.download_url.text().clone();
        let status_state = Arc::clone(&self.compute);
        let temporary_files_path = self.file_temp_path.text().clone();
        


        Some(thread::spawn(move || {
            // Actual process for wipe will run in a background thread to not lock up the UI

            //Wipe the disk
            let wipe_disk_res = ImagingApp::wipe_disk();
            
            let mut inner_state = status_state.write().unwrap();
            *inner_state  = match wipe_disk_res {
                Ok(_) => "DiskWiped".to_string(),
                Err(x) => x
            };
            drop(inner_state);
            sender.notice();

            //Download the WIM
            let download_res = ImagingApp::download_url(url, Path::new(&wim_path));
            
            let mut inner_state = status_state.write().unwrap();
            *inner_state  = match download_res {
                Ok(_) => ("Downloaded").to_string(),
                Err(x) => x
            };
            drop(inner_state);
            sender.notice();

            //Extract the WIM to the disk
            let extract_cmd_result = ImagingApp::apply_wim(&wim_path, &wim_image_index);
            
            let mut inner_state = status_state.write().unwrap();
            *inner_state = match extract_cmd_result {
                Ok(_) => "Extracted".to_string(),
                Err(x) => format!("Error extracting WIM {}",x.raw_os_error().unwrap().to_string()),
            };
            drop(inner_state);
            sender.notice();

            //Remove the WIM
            let _ = fs::remove_file(wim_path.clone());


            //Apply bootloader
            let bootloader_result = ImagingApp::install_bootloader();
            
            let mut inner_state = status_state.write().unwrap();
            *inner_state = match bootloader_result {
                Ok(_) => "Bootloaded".to_string(),
                Err(x) => x,
            };
            drop(inner_state);
            sender.notice();

            //Apply the unattend file
            let _ = ImagingApp::download_and_apply_unattend();

            //Extract any override files
            let apply_files_result = ImagingApp::install_staging_files(&temporary_files_path);
            let mut inner_state = status_state.write().unwrap();
            *inner_state = match apply_files_result {
                Ok(_) => "Staged".to_string(),
                Err(x) => x,
            };
            drop(inner_state);
            sender.notice();

        }));

        //
    }

    fn update_status_bar(self: &ImagingApp) {
        self.status_label.set_text(format!("IP Address - {}; First Disk - {}",
                    ImagingApp::get_first_ip().unwrap_or("Undefined".to_string()),
                    ImagingApp::get_disks().unwrap_or("No disks found".to_string())).as_str());
    }

    fn get_first_ip() -> Result<String,()> {
        let networks = Networks::new_with_refreshed_list();
        for (_interface_name, data) in &networks {
            let networks = data.ip_networks();
            for ipnet in networks {
                //Filter out loopbacks and link locals
                if !ipnet.addr.is_loopback() && !ipnet.addr.is_multicast() && !ipnet.addr.is_unspecified() && !ipnet.addr.to_string().starts_with("169.") && !ipnet.addr.to_string().starts_with("fe80::") && data.total_packets_received() > 1_000 {
                    return Ok(format!("{}/{}",ipnet.addr,ipnet.prefix));
                }
                    
            }
        }
        return  Err(());
    }

    fn get_disks() -> Result<String,()> {
        let disks = Disks::new_with_refreshed_list();
        if let Some( disk) = disks.first() {
            if !disk.is_removable() && !disk.is_read_only() && disk.available_space() > 8_000_000_000  {
                return Ok(format!("{} {} {} GB", disk.name().display(), disk.mount_point().display(), disk.total_space()/1_000_000_000));
            }
        }
        return  Err(());
    }

    fn wait_for_network( timeout: u32, check_url: &String) -> Result<bool,String> {
        let target_time = SystemTime::now() + Duration::new(timeout.into(), 0);
        while !ImagingApp::check_url_valid(check_url.to_string()).is_ok() && SystemTime::now() < target_time {
            sleep( Duration::new(2,0));
        }
        Ok(ImagingApp::check_url_valid(check_url.to_string()).is_ok())
    }
    
    fn is_pe() -> bool {
        Path::new("X:\\").exists() && !Path::new("C:\\Program Files\\WindowsApps").exists()
    }

    fn apply_wim(wim_path: &str, image_index: &u8) -> Result<Output,Error> {
        if ImagingApp::is_pe() {
            Command::new("dism.exe").args(["/Apply-Image",format!("/ImageFile:{}",wim_path).as_str(),format!("/Index:{}",image_index).as_str(),"/ApplyDir:W:\\"]).output()
        }else{
            // Don't apply if we arn't  PE in case someone runs this exe in normal OS load
            Ok(Output { status: ExitStatus::default(), stdout: ([].to_vec()), stderr: ([].to_vec()) })
        }
    }

    fn is_autoinstall() -> bool {
        let mut config = Ini::new();
        if Path::new("Settings.ini").exists() {
            _  = config.load("Settings.ini");
        }
        config.getbool("deploy","auto_install").unwrap_or(Some(false)).unwrap_or(false)
    }

    fn wipe_disk() -> Result<(),String> {
        if ImagingApp::is_pe() {
            let disk_wipe_cmd = Command::new("diskpart.exe").args(["/s","DiskPart.txt"]).output();
            match disk_wipe_cmd {
                Ok(_) => Ok(()),
                Err(e) => Err(format!("Error wiping disk {}",e.raw_os_error().unwrap().to_string()))
            }
        }else{
            // Don't wipe if we arn't  PE in case someone runs this exe in normal OS load
            Ok(())
        }
        
    }

    fn install_bootloader() -> Result<(),String> {
        if ImagingApp::is_pe() {
            let disk_wipe_cmd = Command::new("W:\\Windows\\System32\\bcdboot.exe").args(["W:\\Windows","/s","S:"]).output();
            match disk_wipe_cmd {
                Ok(_) => Ok(()),
                Err(e) => Err(format!("Error installing bootloader {}",e.raw_os_error().unwrap().to_string()))
            }
        }else{
            // Don't wipe if we arn't  PE in case someone runs this exe in normal OS load
            Ok(())
        }
        
    }
    
    fn install_staging_files(tmp_files_path: &String) -> Result<(),String> {
        let mut config = Ini::new();
        if Path::new("Settings.ini").exists() {
            _  = config.load("Settings.ini");
        }

        // Check if the configuration has a copy step to copy from PE (or if the pe shell batch has a net use to copy from a drive)
        match config.get("deploy", "stage_folder") {
            Some(path) => {
                if path.is_empty() {
                    // No path - Assume we don't have anything to copy
                    ()
                }else if Path::new( ImagingApp::sub_system_info_vars(&path).as_str() ).exists() {
                    let copy_path = ImagingApp::sub_system_info_vars(&path);
                    let copy_result = Command::new("xcopy.exe").args([copy_path,"W:\\".to_string(),"/E".to_string(),"/H".to_string(),"/Q".to_string(),"/Y".to_string()]).output();
                    match copy_result {
                        Ok(_) => (),
                        Err(e) => return Err(format!("Error with local copy {} . Tried from {}",e.raw_os_error().unwrap().to_string(),path))
                    };
                }else{
                    return Err(format!("Path for staging files '{}' does not exist",&path))
                }
            },
            None => ()
        }
        let continue_on_http_err = config.getbool("network","download_continue_on_error").unwrap_or(Some(false)).unwrap_or(false);
        // Check if there is a zip we should download and extract
        match config.get("deploy", "stage_download_zip") {
            Some(url) => {
                if url.is_empty() {
                    // No path - Assume we don't have anything to copy
                    ()
                }else {
                    
                    let copy_url = ImagingApp::sub_system_info_vars(&url);

                    //Only throw an error on 40web error  if we don't have a continue on flag set to allow people to still image unknown models.
                    match ImagingApp::check_url_valid(copy_url.clone()) {
                        Ok(false) => {if !continue_on_http_err {return Err(format!("Error with download from {} - Zip may not exist or server may be down.",copy_url))}},
                        Ok(true) => (),
                        Err(_) => return Err(format!("Error with download from {}.",copy_url))
                    }

                    //Make the temporary files path if needed
                    if !Path::new(tmp_files_path).exists() {
                        let _ = fs::create_dir_all(tmp_files_path);
                    }
                    
                    //Download to the staging directory
                    let local_zip_path = format!("{}\\stage.zip",&tmp_files_path);
                    let _ = ImagingApp::download_url(copy_url.clone(), Path::new(&local_zip_path));

                    //Extract them over the OS drive
                    let mut stage_zip_archive = match zip::ZipArchive::new(File::open(&local_zip_path).unwrap()) {
                        Ok(r) => r,
                        Err(e) => return Err(e.to_string())
                    };
                    match stage_zip_archive.extract("W:\\") {
                        Ok(_) => (),
                        Err(e) => return Err(e.to_string())
                    };

                    let _ = fs::remove_file(local_zip_path.clone());


                }
            },
            None => ()
        }

        // Check if there is a zip we should download and extract for drivers
        match config.get("deploy", "drivers_download_zip") {
            Some(url) => {
                if url.is_empty() {
                    // No path - Assume we don't have anything to copy
                    ()
                }else {
                    
                    let copy_url = ImagingApp::sub_system_info_vars(&url);

                    //Only throw an error on 40web error  if we don't have a continue on flag set to allow people to still image unknown models.
                    match ImagingApp::check_url_valid(copy_url.clone()) {
                        Ok(false) => {if !continue_on_http_err {return Err(format!("Error with download from {} - Zip may not exist or server may be down.",copy_url))}},
                        Ok(true) => (),
                        Err(_) => return Err(format!("Error with download from {}.",copy_url))
                    }

                    //Make the temporary files path if needed
                    if !Path::new(tmp_files_path).exists() {
                        let _ = fs::create_dir_all(tmp_files_path);
                    }
                    
                    //Download to the staging directory
                    let local_zip_path = format!("{}\\drivers.zip",&tmp_files_path);
                    let _ = ImagingApp::download_url(copy_url.clone(), Path::new(&local_zip_path));

                    //Extract them over the OS drive
                    let mut stage_zip_archive = match zip::ZipArchive::new(File::open(&local_zip_path).unwrap()) {
                        Ok(r) => r,
                        Err(e) => return Err(e.to_string())
                    };
                    match stage_zip_archive.extract("W:\\") {
                        Ok(_) => (),
                        Err(e) => return Err(e.to_string())
                    };

                    let _ = fs::remove_file(local_zip_path.clone());


                }
            },
            None => ()
        }

        Ok(())
    }

    fn download_and_apply_unattend() -> Result<bool,String> {

        let mut config = Ini::new();
        if Path::new("Settings.ini").exists() {
            _  = config.load("Settings.ini");
        }

        let continue_on_http_err = config.getbool("network","download_continue_on_error").unwrap_or(Some(false)).unwrap_or(false);

         match config.get("deploy", "unattend_download_path") {
            Some(url) => {
                if url.is_empty() {
                    // No path - Assume we don't have anything to copy
                    return Ok(false)
                }else {
                    //Check the file exists
                    let copy_url = ImagingApp::sub_system_info_vars(&url);
                    match ImagingApp::check_url_valid(copy_url.clone()) {
                        Ok(false) => {if !continue_on_http_err {return Err(format!("Error with download of unattend from {} - XML may not exist or may be down.",copy_url))}},
                        Ok(true) => (),
                        Err(_) => return Err(format!("Error with download from {}.",copy_url))
                    }

                    //Create the Panther path if it doesn't exist
                    if !Path::new("W:\\Windows\\Panther\\").exists() {
                        let _ = fs::create_dir_all("W:\\Windows\\Panther\\");
                    }
                    
                    
                    //Download the unattend
                    let local_unattend_path = "W:\\Windows\\Panther\\unattend.xml";
                    let _ = ImagingApp::download_url(copy_url.clone(), Path::new(&local_unattend_path));
                    let apply_result = Command::new("dism.exe").args(["/Image:W:\\".to_string(),format!("/Apply-Unattend:{}",&local_unattend_path).to_string()]).output();
                    match apply_result {
                        Ok(_) => (),
                        Err(e) => return Err(format!("Error with local copy {} . Tried from {}",e.raw_os_error().unwrap().to_string(),&local_unattend_path))
                    };

                    let _ = fs::remove_file(local_unattend_path);
                }
         },
         None => return Ok(false)


        }
        Ok(true)
        
    }

    fn sub_system_info_vars (str: &String) -> String {
        return str.replace("{vendor}", Product::vendor_name().unwrap_or("NO_VENDOR_FOUND".to_string()).as_str())
            .replace("{model}", Product::name().unwrap_or("NO_MODEL_FOUND".to_string()).as_str())
            .replace("{sku}", Product::stock_keeping_unit().unwrap_or("NO_SKU_FOUND".to_string()).as_str())
            .replace("{serial}", Product::serial_number().unwrap_or("NO_SERIAL_FOUND".to_string()).as_str())
    }


    fn check_url_valid(url:String) -> Result<bool,()> {
        Ok(ureq::head(url).call().is_ok())
    }

    fn locate_wim_save_space() -> Result<String,()> {


        let disks = Disks::new_with_refreshed_list();
        let mut path = format!("image.wim");
        for disk in disks.list() {
            if !disk.is_removable() && !disk.is_read_only() && disk.available_space() > 8_000_000_000 {
                println!("[{:?}] {:?}", disk.name(), disk.kind());
                let mount_point = disk.mount_point().to_str().expect("Error getting mount");
                path = format!("{}image.wim",mount_point);
            }
        }
        Ok(path.to_string())
    }


     fn download_url(url:String, local_path: &Path) -> Result<&Path,String> {
        let mut body = match ureq::get(url.clone()).call() {
            Ok(rb) => rb,
            Err(_) => return Err(format!("Error downloading file  {}",url.clone()).to_string())
        };
        let mut body_reader = body.body_mut().as_reader();

        let mut file = match File::create(local_path) {
            Ok(f) => f,
            Err(_) => return Err(format!("Error creating file at {:?}",local_path).to_string())
        };
       
        let res = std::io::copy(&mut body_reader,&mut file);
        match res {
            Ok(_bytes_read) => Ok(local_path),
            Err(_) => Err("Error downloading file".to_string())
        }
    }
  
    fn on_notice(&self) {
        let  data_res = self.compute.try_read();
        let data = match data_res {
            Ok(d) => d,
            Err(e) => {
                nwg::modal_info_message(&self.window, "Error ", &format!("{}",e));
                return;
            }
        };
        match data.as_str() {
            "Downloaded" => { 
                self.install_progress.advance_delta(25); 
                self.install_status.set_text("Unpacking WIM");
            },
            "DiskWiped" => { 
                self.install_progress.advance_delta(5);
                self.install_status.set_text("Downloading");
            },
            "Extracted" => { 
                self.install_progress.advance_delta(25);
                self.install_status.set_text("Installing bootloader");
            },
            "Bootloaded" => { 
                self.install_progress.advance_delta(15);
                self.install_status.set_text("Checking for any staging files");
            },
            "Staged" => { 
                self.install_progress.advance_delta(15);
                self.install_status.set_text("Files are staged, we are ready to reboot");
                if !ImagingApp::is_autoinstall() {
                    nwg::modal_info_message(&self.window, "Imaging Complete", "Click Ok / close this dialog to exit");
                    nwg::stop_thread_dispatch();
                }
            },
            x => {
                nwg::modal_info_message(&self.window, "Error", &format!("Error {}",x));
                self.set_ui(false);
            }
        }
    }

    fn set_ui(&self,running:bool) {
        // Elements that only show when running
        self.download_label.set_visible(running);
        self.install_progress.set_visible(running);
        self.install_status.set_visible(running);

        //UI elements that are part of setup if we are running
        self.start_button.set_visible(!running);
        self.download_url.set_enabled(!running);
        self.wim_temp_path.set_enabled(!running);
        self.file_temp_path.set_enabled(!running);

    }

    fn exit_msg(&self) {
        //nwg::modal_info_message(&self.window, "Goodbye", &format!("Goodbye {}", self.download_url.text()));
        nwg::stop_thread_dispatch();
    }
    
    
}

fn main() {
    
    nwg::init().expect("Failed to initialize the NWG create");
    nwg::Font::set_global_family("Segoe UI").expect("Failed to set Segoe UI font");
    
    let _app = ImagingApp::build_ui(Default::default()).expect("Failed to build UI");
   
    nwg::dispatch_thread_events();
}