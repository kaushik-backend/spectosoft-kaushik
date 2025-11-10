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
use std::{fs, path::PathBuf, thread, time::{Duration, Instant}};
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
fn start_video_capture(state: State<'_, CaptureHandle>, intervalSecs: u64, durationSecs: u64) -> Result<String, String> {
    // Check already running
    if state.video_running.load(Ordering::SeqCst) {
        return Err("Video capture already running".into());
    }

    let output_dir = PathBuf::from("D:\\SpectosoftCaptures\\Videos");
    std::fs::create_dir_all(&output_dir).map_err(|e| e.to_string())?;

    state.video_running.store(true, Ordering::SeqCst);

    let video_running = state.video_running.clone();
    let video_join = state.video_join_handle.clone();

    let handle = thread::spawn(move || {
        println!("🎬 Video capture loop started");
        while video_running.load(Ordering::SeqCst) {
            let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
            let filename = output_dir.join(format!("capture_{}.mp4", timestamp));
            println!("➡️ Recording video to {}", filename.display());

            let ffmpeg_cmd = Command::new("ffmpeg")
                .args([
                    "-y",
                    "-f", "gdigrab",
                    "-framerate", "15",
                    "-draw_mouse", "1",
                    "-offset_x", "0",
                    "-offset_y", "0",
                    "-video_size", "1920x1080",
                    "-show_region", "0",
                    "-i", "desktop",
                    "-t", &durationSecs.to_string(),
                    "-vcodec", "libx264",
                    "-preset", "ultrafast",
                    "-crf", "28",
                    "-pix_fmt", "yuv420p",
                    filename.to_str().unwrap(),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();

            match ffmpeg_cmd {
                Ok(mut child) => {
                    // Wait for ffmpeg to finish the clip or exit early if stopping
                    if let Err(e) = child.wait() {
                        eprintln!("Failed to wait for ffmpeg: {}", e);
                    } else {
                        println!("Saved {}", filename.display());
                    }
                }
                Err(e) => eprintln!("Failed to start ffmpeg: {}", e),
            }

            // sleep, but check video_running in small increments to be responsive to stop
            let mut slept = 0u64;
            let sleep_total = intervalSecs;
            while video_running.load(Ordering::SeqCst) && slept < sleep_total {
                let step = 1u64.min(sleep_total - slept);
                thread::sleep(Duration::from_secs(step));
                slept += step;
            }
        }
        println!("🎬 Video capture loop exiting");
    });

    // store join handle so stop can join
    *video_join.lock().unwrap() = Some(handle);

    Ok("Video capture loop started".into())
}

#[tauri::command]
fn stop_video_capture(state: State<'_, CaptureHandle>) -> Result<String, String> {
    if !state.video_running.load(Ordering::SeqCst) {
        return Err("Video capture not running".into());
    }

    state.video_running.store(false, Ordering::SeqCst);

    if let Some(h) = state.video_join_handle.lock().unwrap().take() {
        let _ = h.join();
    }

    Ok("Video capture stopped".into())
}


/// Get active window + process info with error handling
fn get_active_window_info() -> Option<(String, String, String, u32)> {
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.0 == 0 {
            return None;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        // Window title with bounds checking
        let title_len = GetWindowTextLengthW(hwnd);
        if title_len == 0 || title_len > 1024 {
            return None;
        }
        
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
            process_name.clone(),
            process_name,
            window_title,
            pid,
        ))
    }
}

#[derive(Clone)]
struct CaptureHandle {
    running: Arc<AtomicBool>, // screenshots running
    join_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
    video_running: Arc<AtomicBool>,                     // NEW
    video_join_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>, // NEW
    last_input_ts: Arc<AtomicU64>,
    activity_queue: Arc<Mutex<VecDeque<String>>>,
    log_file_lock: Arc<Mutex<()>>,
}

impl CaptureHandle {
    fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            join_handle: Arc::new(Mutex::new(None)),
            video_running: Arc::new(AtomicBool::new(false)), // NEW
            video_join_handle: Arc::new(Mutex::new(None)),   // NEW
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

/// Input listener with throttling and batching
fn spawn_input_listener(capture_handle: CaptureHandle, logs_dir: &std::path::Path) {
    if let Err(e) = std::fs::create_dir_all(logs_dir) {
        eprintln!("Failed to create logs dir: {}", e);
    }

    let log_path = logs_dir.join("activity.log");
    let last_ts = capture_handle.last_input_ts.clone();
    let queue = capture_handle.activity_queue.clone();
    let file_lock = capture_handle.log_file_lock.clone();

    thread::spawn(move || {
        let mut file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("Cannot open activity.log: {}", e);
                    return;
                }
            };

        let mut metrics = Metrics::default();
        let mut ctrl_pressed = false;
        let mut mouse_pressed = false;
        let mut last_click_time = 0u64;
        let mut last_click_button = Button::Left;
        
        // Throttling for mouse moves to prevent system overload
        let mut last_mouse_log = Instant::now();
        let mut last_log_time = Instant::now();
        const MOUSE_MOVE_THROTTLE_MS: u128 = 100; // Only log mouse moves every 100ms
        const LOG_INTERVAL_MS: u128 = 500; // Batch writes every 500ms
        
        let mut pending_log = false;

        let push_event = |q: &Arc<Mutex<VecDeque<String>>>,
                          file_lock: &Arc<Mutex<()>>,
                          file: &mut std::fs::File,
                          json: String| {
            // Update queue
            if let Ok(mut guard) = q.lock() {
                guard.push_back(json.clone());
                if guard.len() > 200 {
                    guard.pop_front();
                }
            }
            
            // Write to file with error handling
            if let Ok(_fl) = file_lock.lock() {
                if let Err(e) = writeln!(file, "{}", json) {
                    eprintln!("Failed to write to log: {}", e);
                }
                let _ = file.flush();
            }
        };

        let callback = move |event: Event| {
            let ts = current_ts_millis();
            last_ts.store(ts, Ordering::SeqCst);

            let mut should_log = true;

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
                    Key::LeftArrow | Key::RightArrow | Key::UpArrow | Key::DownArrow => {
                        metrics.nav_keys.arrows += 1;
                    }
                    Key::Home | Key::End => metrics.nav_keys.home_end += 1,
                    Key::PageUp | Key::PageDown => metrics.nav_keys.pgup_pgdn += 1,
                    Key::F1 | Key::F2 | Key::F3 | Key::F4 | Key::F5 | Key::F6
                    | Key::F7 | Key::F8 | Key::F9 | Key::F10 | Key::F11 | Key::F12 => {
                        metrics.function_keys.f1_f12 += 1;
                    }
                    Key::KeyC if ctrl_pressed => metrics.copy_count += 1,
                    Key::KeyV if ctrl_pressed => metrics.paste_count += 1,
                    _ => metrics.char_count += 1,
                },
                EventType::KeyRelease(key) => {
                    if matches!(key, Key::ControlLeft | Key::ControlRight) {
                        ctrl_pressed = false;
                    }
                    should_log = false; // Don't log key releases to reduce noise
                }
                EventType::ButtonPress(button) => {
                    mouse_pressed = true;
                    
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
                    should_log = false; // Don't log button releases
                }
                EventType::MouseMove { .. } => {
                    metrics.mouse.moves += 1;
                    
                    if mouse_pressed {
                        metrics.mouse.drags += 1;
                    }
                    
                    // Throttle mouse move logging to prevent system overload
                    let now = Instant::now();
                    if now.duration_since(last_mouse_log).as_millis() < MOUSE_MOVE_THROTTLE_MS {
                        should_log = false;
                    } else {
                        last_mouse_log = now;
                        pending_log = true;
                        should_log = false; // Will log in batch
                    }
                }
                EventType::Wheel { .. } => {
                    metrics.mouse.scrolls += 1;
                }
            }

            // Batch logging to reduce I/O
            if should_log {
                pending_log = true;
            }

            // Only write to log every LOG_INTERVAL_MS or on important events
            let now = Instant::now();
            if pending_log && (should_log || now.duration_since(last_log_time).as_millis() >= LOG_INTERVAL_MS) {
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
                    pending_log = false;
                    last_log_time = now;
                }
            }
        };

        if let Err(e) = listen(callback) {
            eprintln!("rdev error: {:?}", e);
        }
    });
}

#[tauri::command]
fn get_recent_activity(state: State<'_, CaptureHandle>, limit: Option<usize>) -> Vec<String> {
    let limit = limit.unwrap_or(50).min(200); // Cap at 200
    if let Ok(queue) = state.activity_queue.lock() {
        let len = queue.len();
        let start = len.saturating_sub(limit);
        queue.iter().skip(start).cloned().collect()
    } else {
        vec![]
    }
}

#[tauri::command]
fn is_idle(state: State<'_, CaptureHandle>, thresholdSecs: u64) -> bool {
    let last = state.last_input_ts.load(Ordering::SeqCst);
    let now = current_ts_millis();
    now.saturating_sub(last) > (thresholdSecs * 1000)
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
    // Prevent timing attacks with constant-time comparison in production
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
    // outputDir: String,
    intervalSecs: u64,
) -> Result<String, String> {
    if state.running.load(Ordering::SeqCst) {
        return Err("Capture already running".into());
    }

    let out_path = PathBuf::from("D:\\SpectosoftCaptures\\Screenshots");
    fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
    state.running.store(true, Ordering::SeqCst);

    let running = state.running.clone();
    let handle = thread::spawn(move || {
        let display = match Display::primary() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Failed to get display: {:?}", e);
                return;
            }
        };
        
        let mut capturer = match Capturer::new(display) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to create capturer: {:?}", e);
                return;
            }
        };
        
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

            // Pre-allocate buffer with exact size needed
            let mut buf: Vec<u8> = Vec::with_capacity(w * h * 4);
            
            // Convert BGRA to RGBA
            for chunk in frame.chunks_exact(4) {
                buf.push(chunk[2]); // R
                buf.push(chunk[1]); // G
                buf.push(chunk[0]); // B
                buf.push(255);      // A
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
            start_video_capture,
            stop_video_capture,
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri");
}