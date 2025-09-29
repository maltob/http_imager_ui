# HTTP Imager
This tool provides a way to image with a WIM on a webserver from a Windows PE environment. Primarily as a way to allow a simple HTTP Boot imaging without needing an additional tool such as ConfigMgr or MDT.

## Usage
1. Download a release zip with the exe or compile your own with cargo
1. Install Windows ADK with PE Add-on if you have not already or copy a boot.wim from a Windows install DVD
    1. Copy the winpe.wim from WinPE to a temporary folder for editing - this will be in a directory such as `C:\Program Files (x86)\Windows Kits\10\Assessment and Deployment Kit\Windows Preinstallation Environment\amd64\en-us`
1. Create a directory for the wim to be edited in such as `C:\Temp\winpe`
1. Launch command prompt/terminal as Administrator and use DISM to mount the wim for  editing
    `Mount-WindowsImage -ImagePath winpe.wim -Path C:\Temp\WinPE -Index 1`
1. Copy over the EXE as well as the [Settings.ini](example/Settings.ini) and [DiskPart.txt](example/DiskPart.txt) to C:\Temp\WinPE
1. Edit Settings.ini as you see fit
1. Copy over a custom winpeshl.ini to tell Windows PE to configure the network then launch the imaging UI. An example one is provided at [WinPEShl.ini](example/Winpeshl.ini)
1. Save the updated WIM
  `Dismount-WindowsImage -Save -Path C:\Temp\WinPE`
1. Upload the WIM to your HTTP server or media and boot from it to check it works like you expect


  ## Settings.ini reference
#### [storage]
| Setting | Value |
| -- | -- |
|format_script|Defaults to `DiskPart.txt`, but can point to a custom script with different parameters if needed. The example script will place windows on the `W:` drive during PE|
|temp_wim_path|Location to store the WIM, should be large enough to fit the install.wim. Defaults to W: drive|
|temp_path|Location to store temporary files such as the stage.zip during download. Defaults to `W:\windows\temp`|

#### [os]
| Setting | Value |
| -- | -- |
|download_url|Where to download the installed WIM from Ex: `http://127.0.0.1/install.wim`|
|index|The WIM index to apply to the W: drive. Defaults to `1`|

#### [deploy]
| Setting | Value |
| -- | -- |
|auto_install| 0 or 1 to have the install wipe and run atuomatically with no prompts |
|stage_folder|A path to copy from the WIM to the install drive, it will overwrite all existing files. Ex: X:\stage |
|stage_download_zip|URL to download from - supports replacing {vendor} and {model} in URL to allow downloading specific drivers for a model - Ex: http://127.0.0.1/{vendor}-{model}.zip|
|stage_download_continue_on_error|0 or 1 to have the installer for the download zip option above continue the installeven with HTTP error such as the server is down or the file doesn't exist|