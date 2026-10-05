#![windows_subsystem = "windows"]
#![allow(non_snake_case, dead_code)]

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Com::*;
use windows_sys::Win32::System::Power::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub app_type: String, // "win32" or "uwp"
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_base64: Option<String>,
}

const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;

#[link(name = "kernel32")]
extern "system" {
    fn GetStdHandle(nStdHandle: u32) -> HANDLE;
    fn WriteFile(hFile: HANDLE, lpBuffer: *const u8, nNumberOfBytesToWrite: u32, lpNumberOfBytesWritten: *mut u32, lpOverlapped: *mut std::ffi::c_void) -> BOOL;
    fn AttachConsole(dwProcessId: u32) -> BOOL;
    fn GetConsoleWindow() -> HWND;
    fn SetConsoleOutputCP(wCodePageID: u32) -> BOOL;
    fn GetModuleHandleW(lpModuleName: *const u16) -> HMODULE;
    fn GlobalLock(hMem: HGLOBAL) -> *mut std::ffi::c_void;
    fn GlobalUnlock(hMem: HGLOBAL) -> BOOL;
    fn GlobalSize(hMem: HGLOBAL) -> usize;
    fn CreateMutexW(lpMutexAttributes: *mut std::ffi::c_void, bInitialOwner: BOOL, lpName: *const u16) -> HANDLE;
    fn GetLogicalDrives() -> u32;
    fn GetDriveTypeW(lpRootPathName: *const u16) -> u32;
    fn CreateNamedPipeW(
        lpName: *const u16,
        dwOpenMode: u32,
        dwPipeMode: u32,
        nMaxInstances: u32,
        nOutBufferSize: u32,
        nInBufferSize: u32,
        nDefaultTimeOut: u32,
        lpSecurityAttributes: *mut std::ffi::c_void,
    ) -> HANDLE;
    fn ConnectNamedPipe(hNamedPipe: HANDLE, lpOverlapped: *mut std::ffi::c_void) -> BOOL;
    fn DisconnectNamedPipe(hNamedPipe: HANDLE) -> BOOL;
    fn ReadFile(
        hFile: HANDLE,
        lpBuffer: *mut u8,
        nNumberOfBytesToRead: u32,
        lpNumberOfBytesRead: *mut u32,
        lpOverlapped: *mut std::ffi::c_void,
    ) -> BOOL;
}

#[link(name = "mpr")]
extern "system" {
    fn WNetGetConnectionW(lpLocalName: *const u16, lpRemoteName: *mut u16, lpnLength: *mut u32) -> u32;
}

fn query_mapped_network_drives() -> Vec<(String, String)> {
    let mut results = Vec::new();
    unsafe {
        let mask = GetLogicalDrives();
        for i in 0..26 {
            if (mask & (1 << i)) != 0 {
                let letter = (b'A' + i as u8) as char;
                let root_str = format!("{}:\\", letter);
                let wide_root = to_wide(&root_str);
                // 4 = DRIVE_REMOTE
                if GetDriveTypeW(wide_root.as_ptr()) == 4 {
                    let drive_name = format!("{}:", letter);
                    let wide_drive = to_wide(&drive_name);
                    let mut buf = [0u16; 512];
                    let mut len = 512u32;
                    if WNetGetConnectionW(wide_drive.as_ptr(), buf.as_mut_ptr(), &mut len) == 0 {
                        let unc = from_wide(&buf);
                        results.push((drive_name, unc));
                    }
                }
            }
        }
    }
    results
}

#[repr(C)]
struct SHELLEXECUTEINFOW {
    pub cbSize: u32,
    pub fMask: u32,
    pub hwnd: HWND,
    pub lpVerb: *const u16,
    pub lpFile: *const u16,
    pub lpParameters: *const u16,
    pub lpDirectory: *const u16,
    pub nShow: i32,
    pub hInstApp: HINSTANCE,
    pub lpIDList: *mut std::ffi::c_void,
    pub lpClass: *const u16,
    pub hkeyClass: isize,
    pub dwHotKey: u32,
    pub hIconOrMonitor: HANDLE,
    pub hProcess: HANDLE,
}

#[repr(C)]
struct NOTIFYICONDATAW {
    pub cbSize: u32,
    pub hWnd: HWND,
    pub uID: u32,
    pub uFlags: u32,
    pub uCallbackMessage: u32,
    pub hIcon: HICON,
    pub szTip: [u16; 128],
    pub dwState: u32,
    pub dwStateMask: u32,
    pub szInfo: [u16; 256],
    pub uTimeoutOrVersion: u32,
    pub szInfoTitle: [u16; 64],
    pub dwInfoFlags: u32,
    pub guidItem: GUID,
    pub hBalloonIcon: HICON,
}

const NIM_ADD: u32 = 0;
const NIM_DELETE: u32 = 2;
const NIF_MESSAGE: u32 = 0x00000001;
const NIF_ICON: u32 = 0x00000002;
const NIF_TIP: u32 = 0x00000004;

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteExW(pExecInfo: *mut SHELLEXECUTEINFOW) -> BOOL;
    fn Shell_NotifyIconW(dwMessage: u32, lpData: *const NOTIFYICONDATAW) -> BOOL;
}

#[link(name = "user32")]
extern "system" {
    fn ShowWindow(hWnd: HWND, nCmdShow: i32) -> BOOL;
    fn PrivateExtractIconsW(
        szFileName: *const u16,
        nIconIndex: i32,
        cxIcon: i32,
        cyIcon: i32,
        phicon: *mut HICON,
        piconid: *mut u32,
        nIcons: u32,
        flags: u32,
    ) -> u32;
    fn DestroyIcon(hIcon: HICON) -> BOOL;
    fn keybd_event(bVk: u8, bScan: u8, dwFlags: u32, dwExtraInfo: usize);
}

#[link(name = "gdiplus")]
extern "system" {
    fn GdiplusStartup(token: *mut usize, input: *const GdiplusStartupInput, output: *mut std::ffi::c_void) -> i32;
    fn GdiplusShutdown(token: usize);
    fn GdipCreateBitmapFromHICON(hicon: HICON, bitmap: *mut *mut std::ffi::c_void) -> i32;
    fn GdipSaveImageToStream(image: *mut std::ffi::c_void, stream: *mut std::ffi::c_void, clsidEncoder: *const GUID, encoderParams: *const std::ffi::c_void) -> i32;
    fn GdipDisposeImage(image: *mut std::ffi::c_void) -> i32;
}

#[repr(C)]
struct GdiplusStartupInput {
    version: u32,
    debug_event_callback: usize,
    suppress_background_thread: BOOL,
    suppress_external_codecs: BOOL,
}

#[link(name = "ole32")]
extern "system" {
    fn CreateStreamOnHGlobal(hGlobal: HGLOBAL, fDeleteOnRelease: BOOL, ppstm: *mut *mut std::ffi::c_void) -> i32;
    fn GetHGlobalFromStream(pstm: *mut std::ffi::c_void, phglobal: *mut HGLOBAL) -> i32;
}

#[link(name = "crypt32")]
extern "system" {
    fn CryptBinaryToStringA(pbBinary: *const u8, cbBinary: u32, dwFlags: u32, pszString: *mut u8, pcchString: *mut u32) -> BOOL;
}

const CLSID_PNG: GUID = GUID {
    data1: 0x557cf406,
    data2: 0x1a04,
    data3: 0x11d3,
    data4: [0x9a, 0x73, 0x00, 0x00, 0xf8, 0x1e, 0xf3, 0x2e],
};

const CLSID_SHELL_LINK: GUID = GUID {
    data1: 0x00021401,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_ISHELL_LINK_W: GUID = GUID {
    data1: 0x000214F9,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_IPERSIST_FILE: GUID = GUID {
    data1: 0x0000010b,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

#[repr(C)]
struct IShellLinkWVtbl {
    pub QueryInterface: unsafe extern "system" fn(this: *mut std::ffi::c_void, riid: *const GUID, ppvObject: *mut *mut std::ffi::c_void) -> HRESULT,
    pub AddRef: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
    pub Release: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
    pub GetPath: unsafe extern "system" fn(this: *mut std::ffi::c_void, pszFile: *mut u16, cch: i32, pfd: *mut std::ffi::c_void, fFlags: u32) -> HRESULT,
    pub GetIDList: usize,
    pub SetIDList: usize,
    pub GetDescription: usize,
    pub SetDescription: usize,
    pub GetWorkingDirectory: unsafe extern "system" fn(this: *mut std::ffi::c_void, pszDir: *mut u16, cch: i32) -> HRESULT,
    pub SetWorkingDirectory: usize,
    pub GetArguments: unsafe extern "system" fn(this: *mut std::ffi::c_void, pszArgs: *mut u16, cch: i32) -> HRESULT,
    pub SetArguments: usize,
    pub GetHotkey: usize,
    pub SetHotkey: usize,
    pub GetShowCmd: usize,
    pub SetShowCmd: usize,
    pub GetIconLocation: unsafe extern "system" fn(this: *mut std::ffi::c_void, pszIconPath: *mut u16, cch: i32, piIcon: *mut i32) -> HRESULT,
}

#[repr(C)]
struct IPersistFileVtbl {
    pub QueryInterface: unsafe extern "system" fn(this: *mut std::ffi::c_void, riid: *const GUID, ppvObject: *mut *mut std::ffi::c_void) -> HRESULT,
    pub AddRef: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
    pub Release: unsafe extern "system" fn(this: *mut std::ffi::c_void) -> u32,
    pub GetClassID: usize,
    pub IsDirty: usize,
    pub Load: unsafe extern "system" fn(this: *mut std::ffi::c_void, pszFileName: *const u16, dwMode: u32) -> HRESULT,
    pub Save: usize,
    pub SaveCompleted: usize,
    pub GetCurFile: usize,
}

const PIPE_NAME: &str = "\\\\.\\pipe\\remoteapp_launcher";
const WM_TRAYICON: u32 = WM_USER + 1;
const ID_TRAY_EXIT: usize = 1001;
const ID_TRAY_ABOUT: usize = 1002;
const TIMER_KEEPALIVE_ID: usize = 1;

const PIPE_ACCESS_DUPLEX: u32 = 0x00000003;
const PIPE_TYPE_MESSAGE: u32 = 0x00000004;
const PIPE_READMODE_MESSAGE: u32 = 0x00000002;
const PIPE_WAIT: u32 = 0;
const ERROR_PIPE_CONNECTED: u32 = 535;

const SEE_MASK_DOENVSUBST: u32 = 0x00000200;
const SEE_MASK_FLAG_NO_UI: u32 = 0x00000400;

static RUNNING: AtomicBool = AtomicBool::new(true);

fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..len]).to_string_lossy().into_owned()
}

fn extract_icon_base64_png(exe_path: &str, size: i32) -> Option<String> {
    unsafe {
        let wide_path = to_wide(exe_path);
        let mut hicon: HICON = ptr::null_mut();
        let mut icon_id: u32 = 0;
        let count = PrivateExtractIconsW(wide_path.as_ptr(), 0, size, size, &mut hicon, &mut icon_id, 1, 0);
        if count == 0 || hicon.is_null() {
            if size > 64 {
                return extract_icon_base64_png(exe_path, 64);
            }
            return None;
        }

        let mut gp_bitmap: *mut std::ffi::c_void = ptr::null_mut();
        if GdipCreateBitmapFromHICON(hicon, &mut gp_bitmap) != 0 {
            DestroyIcon(hicon);
            return None;
        }

        let mut stream: *mut std::ffi::c_void = ptr::null_mut();
        if CreateStreamOnHGlobal(ptr::null_mut(), 1, &mut stream) != 0 {
            GdipDisposeImage(gp_bitmap);
            DestroyIcon(hicon);
            return None;
        }

        let status = GdipSaveImageToStream(gp_bitmap, stream, &CLSID_PNG, ptr::null());
        GdipDisposeImage(gp_bitmap);
        DestroyIcon(hicon);

        if status != 0 {
            let vtbl = *(stream as *const *const usize);
            let release_fn: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 = std::mem::transmute(*vtbl.add(2));
            release_fn(stream);
            return None;
        }

        let mut hglobal: HGLOBAL = ptr::null_mut();
        GetHGlobalFromStream(stream, &mut hglobal);
        let sz = GlobalSize(hglobal);
        let ptr = GlobalLock(hglobal) as *const u8;

        let mut b64_len = 0u32;
        CryptBinaryToStringA(ptr, sz as u32, 0x00000001 | 0x40000000, ptr::null_mut(), &mut b64_len);

        let mut b64_buf = vec![0u8; b64_len as usize];
        CryptBinaryToStringA(ptr, sz as u32, 0x00000001 | 0x40000000, b64_buf.as_mut_ptr(), &mut b64_len);

        GlobalUnlock(hglobal);
        let vtbl = *(stream as *const *const usize);
        let release_fn: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 = std::mem::transmute(*vtbl.add(2));
        release_fn(stream);

        if let Some(&0) = b64_buf.last() {
            b64_buf.pop();
        }

        String::from_utf8(b64_buf).ok()
    }
}

fn sanitize_id(name: &str) -> String {
    match name {
        "记事本" => return "notepad".to_string(),
        "计算器" => return "calculator".to_string(),
        "终端" => return "terminal".to_string(),
        "设置" => return "settings".to_string(),
        "Windows 安全中心" => return "windows-security".to_string(),
        "Windows 备份" => return "windows-backup".to_string(),
        "图吧工具箱" => return "tb-toolbox".to_string(),
        "UU远程" => return "uu-remote".to_string(),
        "入门" => return "get-started".to_string(),
        "单击以执行" => return "click-to-do".to_string(),
        "Office 语言首选项" => return "office-language-preferences".to_string(),
        _ => {}
    }

    let s: String = name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let trimmed = s.trim_matches('-').to_string();
    if trimmed.is_empty() {
        name.to_lowercase()
    } else {
        trimmed
    }
}

fn parse_lnk_file(sl: *mut std::ffi::c_void, pf: *mut std::ffi::c_void, lnk_path: &Path) -> Option<(String, Option<String>, Option<String>, Option<String>)> {
    unsafe {
        let vtbl_sl = *(sl as *const *const IShellLinkWVtbl);
        let vtbl_pf = *(pf as *const *const IPersistFileVtbl);

        let wide_path = to_wide(&lnk_path.to_string_lossy());
        let hr = ((*vtbl_pf).Load)(pf, wide_path.as_ptr(), 0);
        if hr != 0 {
            return None;
        }

        let mut target_buf = [0u16; 512];
        let mut args_buf = [0u16; 512];
        let mut work_buf = [0u16; 512];
        let mut icon_buf = [0u16; 512];
        let mut icon_idx = 0i32;

        ((*vtbl_sl).GetPath)(sl, target_buf.as_mut_ptr(), 512, ptr::null_mut(), 0);
        ((*vtbl_sl).GetArguments)(sl, args_buf.as_mut_ptr(), 512);
        ((*vtbl_sl).GetWorkingDirectory)(sl, work_buf.as_mut_ptr(), 512);
        ((*vtbl_sl).GetIconLocation)(sl, icon_buf.as_mut_ptr(), 512, &mut icon_idx);

        let target = from_wide(&target_buf);
        if target.is_empty() {
            return None;
        }

        let args = from_wide(&args_buf);
        let work = from_wide(&work_buf);
        let icon = from_wide(&icon_buf);

        Some((
            target,
            if args.is_empty() { None } else { Some(args) },
            if work.is_empty() { None } else { Some(work) },
            if icon.is_empty() { None } else { Some(format!("{},{}", icon, icon_idx)) },
        ))
    }
}

fn scan_shortcuts(with_icons: bool) -> Vec<AppInfo> {
    let mut apps: HashMap<String, AppInfo> = HashMap::new();

    unsafe {
        CoInitialize(ptr::null());
        let mut gdi_token: usize = 0;
        if with_icons {
            let input = GdiplusStartupInput {
                version: 1,
                debug_event_callback: 0,
                suppress_background_thread: 0,
                suppress_external_codecs: 0,
            };
            GdiplusStartup(&mut gdi_token, &input, ptr::null_mut());
        }

        let mut sl: *mut std::ffi::c_void = ptr::null_mut();
        if CoCreateInstance(&CLSID_SHELL_LINK, ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_ISHELL_LINK_W, &mut sl) == 0 {
            let mut pf: *mut std::ffi::c_void = ptr::null_mut();
            let vtbl_sl = *(sl as *const *const IShellLinkWVtbl);
            ((*vtbl_sl).QueryInterface)(sl, &IID_IPERSIST_FILE, &mut pf);

            // 1. Scan Start Menu FIRST (Priority)
            let mut start_menu_dirs = Vec::new();
            if let Ok(progdata) = std::env::var("ProgramData") {
                start_menu_dirs.push(PathBuf::from(progdata).join("Microsoft\\Windows\\Start Menu\\Programs"));
            }
            if let Ok(appdata) = std::env::var("APPDATA") {
                start_menu_dirs.push(PathBuf::from(appdata).join("Microsoft\\Windows\\Start Menu\\Programs"));
            }

            for dir in start_menu_dirs {
                scan_dir_shortcuts(&dir, sl, pf, with_icons, &mut apps, true);
            }

            // 2. Scan Desktop SECOND (De-duplicate: skip if TargetPath already exists)
            let mut desktop_dirs = Vec::new();
            if let Ok(pub_profile) = std::env::var("PUBLIC") {
                desktop_dirs.push(PathBuf::from(pub_profile).join("Desktop"));
            }
            if let Ok(user_profile) = std::env::var("USERPROFILE") {
                desktop_dirs.push(PathBuf::from(user_profile).join("Desktop"));
            }

            for dir in desktop_dirs {
                scan_dir_shortcuts(&dir, sl, pf, with_icons, &mut apps, false);
            }

            let vtbl_pf = *(pf as *const *const IPersistFileVtbl);
            ((*vtbl_pf).Release)(pf);
            ((*vtbl_sl).Release)(sl);
        }

        // 3. Scan UWP / MSIX Apps from Get-StartApps
        scan_uwp_apps(&mut apps);

        if with_icons && gdi_token != 0 {
            GdiplusShutdown(gdi_token);
        }
        CoUninitialize();
    }

    let mut result: Vec<AppInfo> = apps.into_values().collect();
    result.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    result
}

fn scan_uwp_apps(apps: &mut HashMap<String, AppInfo>) {
    use std::process::Command;
    let output = Command::new("powershell.exe")
        .args(&[
            "-NoProfile",
            "-Command",
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; Get-StartApps | Where-Object { $_.AppID -match '!' } | Select-Object Name, AppID | ConvertTo-Json",
        ])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            #[derive(Deserialize)]
            struct StartAppItem {
                Name: String,
                AppID: String,
            }

            let text = String::from_utf8_lossy(&out.stdout);
            if let Ok(items) = serde_json::from_str::<Vec<StartAppItem>>(&text) {
                for item in items {
                    let norm_key = item.AppID.to_lowercase();
                    if !apps.contains_key(&norm_key) {
                        apps.insert(norm_key, AppInfo {
                            id: sanitize_id(&item.Name),
                            name: item.Name,
                            app_type: "uwp".to_string(),
                            target: item.AppID,
                            arguments: None,
                            working_dir: None,
                            icon_base64: None,
                        });
                    }
                }
            } else if let Ok(item) = serde_json::from_str::<StartAppItem>(&text) {
                let norm_key = item.AppID.to_lowercase();
                if !apps.contains_key(&norm_key) {
                    apps.insert(norm_key, AppInfo {
                        id: sanitize_id(&item.Name),
                        name: item.Name,
                        app_type: "uwp".to_string(),
                        target: item.AppID,
                        arguments: None,
                        working_dir: None,
                        icon_base64: None,
                    });
                }
            }
        }
    }
}

fn scan_dir_shortcuts(
    dir: &Path,
    sl: *mut std::ffi::c_void,
    pf: *mut std::ffi::c_void,
    with_icons: bool,
    apps: &mut HashMap<String, AppInfo>,
    is_start_menu: bool,
) {
    if !dir.exists() {
        return;
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                scan_dir_shortcuts(&path, sl, pf, with_icons, apps, is_start_menu);
            } else if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("lnk")).unwrap_or(false) {
                if let Some((target, args, work, icon_loc)) = parse_lnk_file(sl, pf, &path) {
                    let norm_key = target.to_lowercase();
                    if !is_start_menu && apps.contains_key(&norm_key) {
                        continue;
                    }

                    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("App");
                    let name = file_stem.to_string();
                    let id = sanitize_id(&name);

                    let icon_base64 = if with_icons {
                        let icon_target = if let Some(ref loc) = icon_loc {
                            let icon_file = loc.split(',').next().unwrap_or(&target);
                            if icon_file.is_empty() { &target } else { icon_file }
                        } else {
                            &target
                        };
                        extract_icon_base64_png(icon_target, 256)
                    } else {
                        None
                    };

                    apps.insert(norm_key, AppInfo {
                        id,
                        name,
                        app_type: "win32".to_string(),
                        target,
                        arguments: args,
                        working_dir: work,
                        icon_base64,
                    });
                }
            }
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_TRAYICON => {
            if lparam as u32 == WM_RBUTTONUP || lparam as u32 == WM_CONTEXTMENU {
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);

                let hmenu = CreatePopupMenu();
                let about_text = to_wide("关于 (&About)");
                let exit_text = to_wide("退出会话 (&Exit)");

                AppendMenuW(hmenu, MF_STRING, ID_TRAY_ABOUT, about_text.as_ptr());
                AppendMenuW(hmenu, MF_SEPARATOR, 0, ptr::null());
                AppendMenuW(hmenu, MF_STRING, ID_TRAY_EXIT, exit_text.as_ptr());

                SetForegroundWindow(hwnd);
                TrackPopupMenu(hmenu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, ptr::null());
                DestroyMenu(hmenu);
                return 0;
            }
        }
        WM_COMMAND => {
            let id = wparam & 0xFFFF;
            if id == ID_TRAY_EXIT {
                                DestroyWindow(hwnd);
                return 0;
            } else if id == ID_TRAY_ABOUT {
                let text = to_wide("RemoteApp 守护保活进程\n用于在单个 RDP Session 中维持会话并拉起应用。");
                let title = to_wide("RemoteApp Launcher");
                MessageBoxW(hwnd, text.as_ptr(), title.as_ptr(), MB_ICONINFORMATION | MB_OK);
                return 0;
            }
        }
        WM_TIMER => {
            if wparam == TIMER_KEEPALIVE_ID {
                SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_AWAYMODE_REQUIRED | ES_DISPLAY_REQUIRED);
                keybd_event(0x87, 0, 0, 0); // VK_F24 down
                keybd_event(0x87, 0, 2, 0); // VK_F24 up
                return 0;
            }
        }
        WM_DESTROY => {
            RUNNING.store(false, Ordering::SeqCst);
            PostQuitMessage(0);
            return 0;
        }
        _ => {}
    }

    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[repr(C)]
#[derive(Copy, Clone)]
struct MIB_TCPROW_OWNER_PID {
    pub dwState: u32,
    pub dwLocalAddr: u32,
    pub dwLocalPort: u32,
    pub dwRemoteAddr: u32,
    pub dwRemotePort: u32,
    pub dwOwningPid: u32,
}

#[repr(C)]
struct MIB_TCPTABLE_OWNER_PID {
    pub dwNumEntries: u32,
    pub table: [MIB_TCPROW_OWNER_PID; 1],
}

#[link(name = "iphlpapi")]
extern "system" {
    fn GetExtendedTcpTable(
        pTcpTable: *mut std::ffi::c_void,
        pdwSize: *mut u32,
        bOrder: BOOL,
        ulAf: u32,
        TableClass: u32,
        Reserved: u32,
    ) -> u32;
}

fn count_active_ssh_connections() -> usize {
    let mut count = 0;
    unsafe {
        // 1. Query IPv4 TCP Table (AF_INET = 2, TCP_TABLE_OWNER_PID_ALL = 5)
        let mut size = 0u32;
        let _ = GetExtendedTcpTable(ptr::null_mut(), &mut size, 0, 2, 5, 0);
        if size > 0 {
            let mut buf: Vec<u8> = vec![0; size as usize];
            if GetExtendedTcpTable(buf.as_mut_ptr() as *mut _, &mut size, 0, 2, 5, 0) == 0 {
                let p_table = buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID;
                let num_entries = (*p_table).dwNumEntries as usize;
                let p_rows = buf.as_ptr().add(std::mem::size_of::<u32>()) as *const MIB_TCPROW_OWNER_PID;
                for i in 0..num_entries {
                    let row = *p_rows.add(i);
                    let local_port = u16::from_be((row.dwLocalPort & 0xFFFF) as u16);
                    if local_port == 22 && row.dwState == 5 /* MIB_TCP_STATE_ESTAB */ {
                        count += 1;
                    }
                }
            }
        }

        // 2. Query IPv6 TCP Table (AF_INET6 = 23, TCP_TABLE_OWNER_PID_ALL = 5)
        let mut size_v6 = 0u32;
        let _ = GetExtendedTcpTable(ptr::null_mut(), &mut size_v6, 0, 23, 5, 0);
        if size_v6 > 0 {
            let mut buf_v6: Vec<u8> = vec![0; size_v6 as usize];
            if GetExtendedTcpTable(buf_v6.as_mut_ptr() as *mut _, &mut size_v6, 0, 23, 5, 0) == 0 {
                let num_entries = *(buf_v6.as_ptr() as *const u32) as usize;
                let row_offset = std::mem::size_of::<u32>();
                let row_size = 56usize; // 16 local + 4 scope + 4 port + 16 remote + 4 scope + 4 port + 4 state + 4 pid
                for i in 0..num_entries {
                    let ptr = buf_v6.as_ptr().add(row_offset + i * row_size);
                    let local_port_raw = *(ptr.add(20) as *const u32);
                    let state = *(ptr.add(48) as *const u32);
                    let local_port = u16::from_be((local_port_raw & 0xFFFF) as u16);
                    if local_port == 22 && state == 5 {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

const DEFAULT_TCP_PORT: u16 = 49152;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "action")]
pub enum AgentRequest {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "run")]
    Run { target: String, params: Option<String> },
    #[serde(rename = "open")]
    Open { file: String },
    #[serde(rename = "list_apps")]
    ListApps {
        #[serde(default)]
        with_icons: bool,
    },
    #[serde(rename = "get_mapped_drives")]
    GetMappedDrives,
    #[serde(rename = "quit")]
    Quit,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AgentResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<AppInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_ssh_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drives: Option<Vec<(String, String)>>,
}

fn handle_tcp_client(stream: &mut std::net::TcpStream, hwnd: HWND) {
    use std::io::{BufRead, BufReader, Write};

    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    });
    let mut line = String::new();

    if reader.read_line(&mut line).is_ok() && !line.trim().is_empty() {
        let req_res: Result<AgentRequest, _> = serde_json::from_str(line.trim());
        let response = match req_res {
            Ok(AgentRequest::Ping) | Ok(AgentRequest::Status) => {
                let ssh_count = count_active_ssh_connections();
                AgentResponse {
                    status: "ok".to_string(),
                    message: Some("pong".to_string()),
                    apps: None,
                    active_ssh_count: Some(ssh_count),
                    drives: None,
                }
            }
            Ok(AgentRequest::Run { target, params }) => {
                launch_application(&target, params.as_deref());
                AgentResponse {
                    status: "ok".to_string(),
                    message: None,
                    apps: None,
                    active_ssh_count: None,
                    drives: None,
                }
            }
            Ok(AgentRequest::Open { file }) => {
                launch_application(&file, None);
                AgentResponse {
                    status: "ok".to_string(),
                    message: None,
                    apps: None,
                    active_ssh_count: None,
                    drives: None,
                }
            }
            Ok(AgentRequest::ListApps { with_icons }) => {
                let apps = scan_shortcuts(with_icons);
                AgentResponse {
                    status: "ok".to_string(),
                    message: None,
                    apps: Some(apps),
                    active_ssh_count: None,
                    drives: None,
                }
            }
            Ok(AgentRequest::GetMappedDrives) => {
                let drives = query_mapped_network_drives();
                AgentResponse {
                    status: "ok".to_string(),
                    message: None,
                    apps: None,
                    active_ssh_count: None,
                    drives: Some(drives),
                }
            }
            Ok(AgentRequest::Quit) => {
                unsafe {
                    PostMessageW(hwnd, WM_DESTROY, 0, 0);
                }
                AgentResponse {
                    status: "ok".to_string(),
                    message: Some("quitting".to_string()),
                    apps: None,
                    active_ssh_count: None,
                    drives: None,
                }
            }
            Err(e) => AgentResponse {
                status: "error".to_string(),
                message: Some(format!("Invalid request: {}", e)),
                apps: None,
                active_ssh_count: None,
                drives: None,
            },
        };

        if let Ok(resp_json) = serde_json::to_string(&response) {
            let _ = stream.write_all(resp_json.as_bytes());
            let _ = stream.write_all(b"\n");
            let _ = stream.flush();
        }
    }
}

enum LaunchTask {
    Launch { target: String, params: Option<String> },
    Quit,
}

fn run_daemon() {
    unsafe {
        let chwnd = GetConsoleWindow();
        if !chwnd.is_null() {
            ShowWindow(chwnd, 0); // SW_HIDE
        }
        let class_name = to_wide("RemoteAppLauncherClass");
        let window_name = to_wide("RemoteApp Launcher Daemon");

        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(ptr::null()),
            hIcon: LoadIconW(ptr::null_mut(), IDI_APPLICATION),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };

        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_name.as_ptr(),
            0,
            0, 0, 0, 0,
            -3 as isize as HWND, // HWND_MESSAGE (message-only window)
            ptr::null_mut(),
            wc.hInstance,
            ptr::null_mut(),
        );

        if hwnd.is_null() {
            eprintln!("Failed to create daemon message window");
            return;
        }

        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAYICON;
        nid.hIcon = LoadIconW(ptr::null_mut(), IDI_APPLICATION);

        let tip = to_wide("RemoteApp 保活中 (FreeRDP)");
        let len = tip.len().min(nid.szTip.len());
        nid.szTip[..len].copy_from_slice(&tip[..len]);

        Shell_NotifyIconW(NIM_ADD, &nid);

        SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_AWAYMODE_REQUIRED | ES_DISPLAY_REQUIRED);
        SetTimer(hwnd, TIMER_KEEPALIVE_ID, 60_000, None);

        // TCP server listener thread (Port 49152) for direct low-latency commands from Linux host
        let hwnd_for_tcp = hwnd as usize;
        std::thread::spawn(move || {
            let addr = format!("0.0.0.0:{}", DEFAULT_TCP_PORT);
            if let Ok(listener) = std::net::TcpListener::bind(&addr) {
                for stream in listener.incoming() {
                    if !RUNNING.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Ok(mut s) = stream {
                        handle_tcp_client(&mut s, hwnd_for_tcp as HWND);
                    }
                }
            } else {
                eprintln!("Failed to bind TCP listener on {}", addr);
            }
        });

        // Sequential FIFO task queue: multiple commands are processed strictly in arrival order
        let (task_tx, task_rx) = std::sync::mpsc::channel::<LaunchTask>();
        std::thread::spawn(move || {
            while let Ok(task) = task_rx.recv() {
                match task {
                    LaunchTask::Launch { target, params } => {
                        launch_application(&target, params.as_deref());
                        std::thread::sleep(std::time::Duration::from_millis(150));
                    }
                    LaunchTask::Quit => break,
                }
            }
        });

        let hwnd_for_pipe = hwnd as usize;
        let pipe_tx = task_tx.clone();
        std::thread::spawn(move || {
            let pipe_w = to_wide(PIPE_NAME);
            while RUNNING.load(Ordering::SeqCst) {
                let hpipe = CreateNamedPipeW(
                    pipe_w.as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                    1,
                    4096,
                    4096,
                    0,
                    ptr::null_mut(),
                );

                if hpipe == INVALID_HANDLE_VALUE {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                }

                let connected = ConnectNamedPipe(hpipe, ptr::null_mut());
                if connected != 0 || GetLastError() == ERROR_PIPE_CONNECTED {
                    let mut buf = [0u8; 4096];
                    let mut bytes_read = 0u32;
                    if ReadFile(hpipe, buf.as_mut_ptr(), 4096, &mut bytes_read, ptr::null_mut()) != 0 && bytes_read > 0 {
                        let msg = String::from_utf8_lossy(&buf[..bytes_read as usize]).trim().to_string();
                        if msg == "PING" {
                            // Health check probe, ignore without spawning any process
                        } else if msg.starts_with("RUN ") {
                            let cmd_to_run = msg[4..].trim().to_string();
                            let _ = pipe_tx.send(LaunchTask::Launch { target: cmd_to_run, params: None });
                        } else if msg.starts_with("RUN_PARAMS ") {
                            let payload = msg[11..].trim().to_string();
                            if let Some((app_file, params)) = payload.split_once('\t') {
                                let _ = pipe_tx.send(LaunchTask::Launch {
                                    target: app_file.to_string(),
                                    params: Some(params.to_string()),
                                });
                            }
                        } else if msg == "QUIT" || msg == "STOP" {
                            let _ = pipe_tx.send(LaunchTask::Quit);
                            PostMessageW(hwnd_for_pipe as HWND, WM_DESTROY, 0, 0);
                            DisconnectNamedPipe(hpipe);
                            CloseHandle(hpipe);
                            break;
                        }
                    }
                }
                DisconnectNamedPipe(hpipe);
                CloseHandle(hpipe);
            }
        });

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn launch_application(target: &str, params_opt: Option<&str>) {
    unsafe {
        let clean_target = target.trim().trim_matches('"');
        if clean_target.contains('!') {
            let explorer = to_wide("explorer.exe");
            let param = to_wide(&format!("shell:AppsFolder\\{}", clean_target));
            let open_verb = to_wide("open");

            let mut sei: SHELLEXECUTEINFOW = std::mem::zeroed();
            sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            sei.fMask = SEE_MASK_DOENVSUBST | SEE_MASK_FLAG_NO_UI;
            sei.lpVerb = open_verb.as_ptr();
            sei.lpFile = explorer.as_ptr();
            sei.lpParameters = param.as_ptr();
            sei.nShow = SW_SHOWNORMAL;

            ShellExecuteExW(&mut sei);
            return;
        }

        let wide_file = to_wide(clean_target);
        let wide_params = params_opt.map(to_wide);
        let open_verb = to_wide("open");

        let mut sei: SHELLEXECUTEINFOW = std::mem::zeroed();
        sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        sei.fMask = SEE_MASK_DOENVSUBST | SEE_MASK_FLAG_NO_UI;
        sei.lpVerb = open_verb.as_ptr();
        sei.lpFile = wide_file.as_ptr();
        if let Some(ref p) = wide_params {
            sei.lpParameters = p.as_ptr();
        }
        sei.nShow = SW_SHOWNORMAL;

        ShellExecuteExW(&mut sei);
    }
}

fn resolve_target_and_params(args: &[String]) -> (String, Option<String>) {
    let raw_joined = args.join(" ");
    let clean_joined = raw_joined.trim().trim_matches('"');

    // 1. If the entire string is an existing file or directory on disk, it's a single target
    if Path::new(clean_joined).exists() {
        return (clean_joined.to_string(), None);
    }

    // 2. Try to find an existing executable prefix among arguments
    for i in (1..args.len()).rev() {
        let prefix = args[..i].join(" ");
        let clean_prefix = prefix.trim().trim_matches('"');
        if Path::new(clean_prefix).is_file() {
            let params = args[i..].join(" ");
            let clean_params = params.trim();
            return (clean_prefix.to_string(), if clean_params.is_empty() { None } else { Some(clean_params.to_string()) });
        }
    }

    // 3. If first argument ends with .exe, assume first argument is binary and rest are params
    if let Some(first) = args.first() {
        let clean_first = first.trim().trim_matches('"');
        if clean_first.to_lowercase().ends_with(".exe") || clean_first.to_lowercase().ends_with(".cmd") || clean_first.to_lowercase().ends_with(".bat") {
            let params = args[1..].join(" ");
            let clean_params = params.trim();
            return (clean_first.to_string(), if clean_params.is_empty() { None } else { Some(clean_params.to_string()) });
        }
    }

    // 4. Default fallback: treat entire string as target
    (clean_joined.to_string(), None)
}

fn send_pipe_command(command: &str) -> bool {
    use std::fs::OpenOptions;
    use std::io::Write;

    if let Ok(mut file) = OpenOptions::new().write(true).open(PIPE_NAME) {
        if file.write_all(command.as_bytes()).is_ok() {
            let _ = file.flush();
            return true;
        }
    }
    false
}

fn main() {
    unsafe {
        AttachConsole(0xFFFFFFFF);
        SetConsoleOutputCP(65001);
    }
    let args: Vec<String> = std::env::args().collect();
    let subcommand = args.get(1).map(|s| s.as_str()).unwrap_or("daemon");

    // Enforce strict singleton for the daemon instance in this session
    if subcommand == "daemon" {
        unsafe {
            let mutex_name = to_wide("Local\\RemoteAppLauncher_Singleton_Mutex");
            let h_mutex = CreateMutexW(ptr::null_mut(), FALSE, mutex_name.as_ptr());
            if h_mutex.is_null() || GetLastError() == ERROR_ALREADY_EXISTS {
                // Another instance is already running; exit silently
                std::process::exit(0);
            }
        }
    }

    match subcommand {
        "list-apps" => {
            let with_icons = args.iter().any(|a| a == "--with-icons");
            let apps = scan_shortcuts(with_icons);
            if let Ok(json) = serde_json::to_string_pretty(&apps) {
                print_to_stdout(&json); print_to_stdout("\r\n");
            }
        }
        "check-ssh" => {
            let count = count_active_ssh_connections();
            print_to_stdout(&format!("Active SSH connections: {}\r\n", count));
            std::process::exit(0);
        }
        "ping" => {
            if send_pipe_command("PING") {
                print_to_stdout("PONG\r\n");
                std::process::exit(0);
            } else {
                std::process::exit(1);
            }
        }
        "run" => {
            if args.len() > 2 {
                let (target, params_opt) = resolve_target_and_params(&args[2..]);
                let msg = if let Some(ref params) = params_opt {
                    format!("RUN_PARAMS {}\t{}", target, params)
                } else {
                    format!("RUN {}", target)
                };
                if send_pipe_command(&msg) {
                    print_to_stdout(&format!("OK: {}\r\n", msg));
                    std::process::exit(0);
                } else {
                    eprintln!("Daemon pipe not ready");
                    std::process::exit(1);
                }
            } else {
                eprintln!("Usage: remoteapp-launcher.exe run <path_or_cmd> [args...]");
                std::process::exit(1);
            }
        }
        "stop" | "quit" => {
            if send_pipe_command("QUIT") {
                print_to_stdout("RemoteApp daemon stopping...\r\n");
            } else {
                eprintln!("RemoteApp daemon is not running.");
            }
        }
        "daemon" | _ => {
            run_daemon();
        }
    }
}

fn print_to_stdout(s: &str) {
    unsafe {
        let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
        if !stdout.is_null() && stdout != INVALID_HANDLE_VALUE {
            let bytes = s.as_bytes();
            let mut written = 0u32;
            WriteFile(stdout, bytes.as_ptr(), bytes.len() as u32, &mut written, ptr::null_mut());
        }
    }
}
