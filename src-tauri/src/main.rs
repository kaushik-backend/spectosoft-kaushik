#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::{Command, Stdio};
use chrono::Local;
use image::{ImageBuffer, Rgba};
use rdev::{listen, Event, EventType, Key, Button};
use scrap::{Capturer, Display};
use serde::Serialize;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::{fs, path::PathBuf, thread, time::Duration};
use tauri::State;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use windows::Win32::{
    Foundation::HWND,
    System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    },
    UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    },
};

#[tauri::command]
fn start_video_capture(outputDir: String, intervalSecs: u64, durationSecs: u64) -> Result<String, String> {
    std::fs::create_dir_all(&outputDir).map_err(|e| e.to_string())?;

    thread::spawn(move || {
        loop {
            let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
            let filename = format!("{}/capture_{}.mp4", outputDir, timestamp);

            // Record video for `duration_secs`
            let ffmpeg_cmd = Command::new("ffmpeg")
                .args([
                    "-y", // overwrite output
                    "-f", "gdigrab", // screen capture for Windows
                    "-framerate", "15", // 15 fps
                    "-i", "desktop", // entire desktop
                    "-t", &durationSecs.to_string(), // record length
                    "-vcodec", "libx264", // H.264 compression
                    "-preset", "veryfast", // tradeoff: faster encoding, larger size
                    "-crf", "30", // quality 0–51 (lower = better)
                    &filename,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();

            match ffmpeg_cmd {
                Ok(mut child) => {
                    let _ = child.wait();
                    println!("Saved {}", filename);
                }
                Err(e) => eprintln!("Failed to start ffmpeg: {}", e),
            }

            thread::sleep(Duration::from_secs(intervalSecs));
        }
    });

    Ok("Video capture loop started".into())
}

///  Get active window + process info
fn get_active_window_info() -> Option<(String, String, String, u32)> {
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.0 == 0 {
            return None;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        // Window title
        let title_len = GetWindowTextLengthW(hwnd);
        let mut buffer = vec![0u16; (title_len + 1) as usize];
        GetWindowTextW(hwnd, &mut buffer);
        let window_title = OsString::from_wide(&buffer)
            .to_string_lossy()
            .trim_end_matches('\0')
            .to_string();

        // Process name
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W::default();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        let mut process_name = "Unknown".to_string();

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                if entry.th32ProcessID == pid {
                    let len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let name = OsString::from_wide(&entry.szExeFile[..len]);
                    process_name = name.to_string_lossy().to_string();
                    break;
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        Some((
            process_name.clone(), // app_name
            process_name,         // process_name
            window_title,         // window title
            pid,
        ))
    }
}

#[derive(Clone)]
struct CaptureHandle {
    running: Arc<AtomicBool>,
    join_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    last_input_ts: Arc<AtomicU64>,
    activity_queue: Arc<Mutex<VecDeque<String>>>,
    log_file_lock: Arc<Mutex<()>>,
}

impl CaptureHandle {
    fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            join_handle: Arc::new(Mutex::new(None)),
            last_input_ts: Arc::new(AtomicU64::new(current_ts_millis())),
            activity_queue: Arc::new(Mutex::new(VecDeque::with_capacity(200))),
            log_file_lock: Arc::new(Mutex::new(())),
        }
    }
}

fn current_ts_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Default)]
struct Metrics {
    kpm: u64,
    char_count: u64,
    backspace_count: u64,
    enter_count: u64,
    copy_count: u64,
    paste_count: u64,
    mods: ModStats,
    nav_keys: NavKeys,
    function_keys: FunctionKeys,
    mouse: MouseStats,
}

#[derive(Debug, Clone, Serialize, Default)]
struct ModStats {
    alt: u64,
    shift: u64,
    ctrl: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
struct NavKeys {
    pgup_pgdn: u64,
    arrows: u64,
    home_end: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
struct FunctionKeys {
    f1_f12: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
struct MouseStats {
    left_clicks: u64,
    right_clicks: u64,
    middle_clicks: u64,
    scrolls: u64,
    moves: u64,
    double_clicks: u64,
    drags: u64,
}

///  Input listener — logs key events, mouse events + window info
fn spawn_input_listener(capture_handle: CaptureHandle, logs_dir: &std::path::Path) {
    if let Err(e) = std::fs::create_dir_all(logs_dir) {
        eprintln!("Failed to create logs dir: {}", e);
    }

    let log_path = logs_dir.join("activity.log");
    let last_ts = capture_handle.last_input_ts.clone();
    let queue = capture_handle.activity_queue.clone();
    let file_lock = capture_handle.log_file_lock.clone();

    thread::spawn(move || {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .expect("Cannot open activity.log");

        let mut metrics = Metrics::default();
        let mut ctrl_pressed = false;
        let mut mouse_pressed = false;
        let mut last_click_time = 0u64;
        let mut last_click_button = Button::Left;

        let push_event = |q: &Arc<Mutex<VecDeque<String>>>,
                          file_lock: &Arc<Mutex<()>>,
                          file: &mut std::fs::File,
                          json: String| {
            if let Ok(mut guard) = q.lock() {
                guard.push_back(json.clone());
                if guard.len() > 200 {
                    guard.pop_front();
                }
            }
            if let Ok(_fl) = file_lock.lock() {
                let _ = writeln!(file, "{}", json);
                let _ = file.flush();
            }
        };

        let callback = move |event: Event| {
            let ts = current_ts_millis();
            last_ts.store(ts, Ordering::SeqCst);

            match event.event_type {
                EventType::KeyPress(key) => match key {
                    Key::ControlLeft | Key::ControlRight => {
                        ctrl_pressed = true;
                        metrics.mods.ctrl += 1;
                    }
                    Key::Alt | Key::AltGr => metrics.mods.alt += 1,
                    Key::ShiftLeft | Key::ShiftRight => metrics.mods.shift += 1,
                    Key::Return => metrics.enter_count += 1,
                    Key::Backspace => metrics.backspace_count += 1,
                    Key::LeftArrow
                    | Key::RightArrow
                    | Key::UpArrow
                    | Key::DownArrow => metrics.nav_keys.arrows += 1,
                    Key::Home | Key::End => metrics.nav_keys.home_end += 1,
                    Key::PageUp | Key::PageDown => metrics.nav_keys.pgup_pgdn += 1,
                    Key::F1
                    | Key::F2
                    | Key::F3
                    | Key::F4
                    | Key::F5
                    | Key::F6
                    | Key::F7
                    | Key::F8
                    | Key::F9
                    | Key::F10
                    | Key::F11
                    | Key::F12 => metrics.function_keys.f1_f12 += 1,
                    Key::KeyC if ctrl_pressed => metrics.copy_count += 1,
                    Key::KeyV if ctrl_pressed => metrics.paste_count += 1,
                    _ => metrics.char_count += 1,
                },
                EventType::KeyRelease(key) => {
                    if matches!(key, Key::ControlLeft | Key::ControlRight) {
                        ctrl_pressed = false;
                    }
                }
                EventType::ButtonPress(button) => {
                    mouse_pressed = true;
                    
                    // Detect double-click (within 500ms)
                    if ts - last_click_time < 500 && button == last_click_button {
                        metrics.mouse.double_clicks += 1;
                    }
                    
                    last_click_time = ts;
                    last_click_button = button;
                    
                    match button {
                        Button::Left => metrics.mouse.left_clicks += 1,
                        Button::Right => metrics.mouse.right_clicks += 1,
                        Button::Middle => metrics.mouse.middle_clicks += 1,
                        _ => {}
                    }
                }
                EventType::ButtonRelease(_) => {
                    mouse_pressed = false;
                }
                EventType::MouseMove { .. } => {
                    metrics.mouse.moves += 1;
                    
                    // Track drags (mouse move while button pressed)
                    if mouse_pressed {
                        metrics.mouse.drags += 1;
                    }
                }
                EventType::Wheel { .. } => {
                    metrics.mouse.scrolls += 1;
                }
            }

            if let Some((app, process, title, pid)) = get_active_window_info() {
                let json = serde_json::json!({
                    "app_name": app,
                    "window_title": title,
                    "process_name": process,
                    "pid": pid,
                    "timestamp": Local::now().to_rfc3339(),
                    "metrics": metrics
                })
                .to_string();
                push_event(&queue, &file_lock, &mut file, json);
            }
        };

        if let Err(e) = listen(callback) {
            eprintln!("rdev error: {:?}", e);
        }
    });
}

#[tauri::command]
fn get_recent_activity(state: State<'_, CaptureHandle>, limit: Option<usize>) -> Vec<String> {
    let limit = limit.unwrap_or(50);
    if let Ok(queue) = state.activity_queue.lock() {
        let len = queue.len();
        let start = len.saturating_sub(limit);
        queue.iter().skip(start).cloned().collect()
    } else {
        vec![]
    }
}

#[tauri::command]
fn is_idle(state: State<'_, CaptureHandle>, threshold_secs: u64) -> bool {
    let last = state.last_input_ts.load(Ordering::SeqCst);
    let now = current_ts_millis();
    now.saturating_sub(last) > (threshold_secs * 1000)
}

#[tauri::command]
fn clear_activity(state: State<'_, CaptureHandle>) {
    if let Ok(mut q) = state.activity_queue.lock() {
        q.clear();
    }
}

#[derive(Serialize)]
struct LoginResponse {
    success: bool,
    message: String,
}

#[tauri::command]
fn login(username: String, password: String) -> LoginResponse {
    if username == "admin" && password == "password123" {
        LoginResponse {
            success: true,
            message: "Login successful".into(),
        }
    } else {
        LoginResponse {
            success: false,
            message: "Invalid credentials".into(),
        }
    }
}

#[tauri::command]
fn start_capture(
    state: State<'_, CaptureHandle>,
    outputDir: String,
    intervalSecs: u64,
) -> Result<String, String> {
    if state.running.load(Ordering::SeqCst) {
        return Err("Capture already running".into());
    }

    let out_path = PathBuf::from(&outputDir);
    fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
    state.running.store(true, Ordering::SeqCst);

    let running = state.running.clone();
    let handle = thread::spawn(move || {
        let display = Display::primary().expect("no display");
        let mut capturer = Capturer::new(display).expect("cannot capture");
        let w = capturer.width();
        let h = capturer.height();

        while running.load(Ordering::SeqCst) {
            let frame = loop {
                match capturer.frame() {
                    Ok(b) => break b.to_vec(),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(e) => {
                        eprintln!("Capture error: {:?}", e);
                        return;
                    }
                }
            };

            let mut buf: Vec<u8> = Vec::with_capacity(w * h * 4);
            for chunk in frame.chunks_exact(4) {
                buf.extend_from_slice(&[chunk[2], chunk[1], chunk[0], 255]);
            }

            if let Some(img) = ImageBuffer::<Rgba<u8>, _>::from_raw(w as u32, h as u32, buf) {
                let ts = Local::now().timestamp_millis();
                let path = out_path.join(format!("screenshot_{}.png", ts));
                if let Err(e) = img.save(&path) {
                    eprintln!("Save failed: {}", e);
                }
            }
            thread::sleep(Duration::from_secs(intervalSecs.max(1)));
        }
    });

    *state.join_handle.lock().unwrap() = Some(handle);
    Ok("Capture started".into())
}

#[tauri::command]
fn stop_capture(state: State<'_, CaptureHandle>) -> Result<String, String> {
    state.running.store(false, Ordering::SeqCst);
    if let Some(h) = state.join_handle.lock().unwrap().take() {
        let _ = h.join();
    }
    Ok("Capture stopped".into())
}

#[tauri::command]
fn capture_status(state: State<'_, CaptureHandle>) -> bool {
    state.running.load(Ordering::SeqCst)
}

fn main() {
    let capture_handle = CaptureHandle::new();
    spawn_input_listener(capture_handle.clone(), std::path::Path::new("logs"));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(capture_handle)
        .invoke_handler(tauri::generate_handler![
            login,
            start_capture,
            stop_capture,
            capture_status,
            is_idle,
            get_recent_activity,
            clear_activity,
            start_video_capture
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri");
}