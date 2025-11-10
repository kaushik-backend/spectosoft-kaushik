import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { message } from "@tauri-apps/plugin-dialog";

export default function App() {
  const [user, setUser] = useState({ username: "", password: "" });
  const [loggedIn, setLoggedIn] = useState(false);
  const [status, setStatus] = useState(false);
  const [intervalSec, setIntervalSec] = useState(5);
  const [outputDir, setOutputDir] = useState("");
  const [activity, setActivity] = useState([]);
  const [isIdle, setIsIdle] = useState(false);
  const [latestMetrics, setLatestMetrics] = useState(null);

  // ✅ Login
  async function onLogin() {
    try {
      const res = await invoke("login", user);
      if (res.success) {
        setLoggedIn(true);
        alert(res.message);
      } else {
        alert(res.message);
      }
    } catch (err) {
      console.error(err);
      alert("Login failed: " + err.message);
    }
  }

  async function onStartVideoCapture() {
    // if (!outputDir) {
    //   alert("Enter output folder (e.g. D:\\TauriCaptures)");
    //   return;
    // }
    try {
      const msg = await invoke("start_video_capture", {
        // output_dir: outputDir,  // Changed to snake_case
        intervalSecs: Number(intervalSec),  // Changed to snake_case
        durationSecs: Number(30),  // Changed to snake_case
      });
      console.log("msg", msg);
     await message("Video capture started successfully!", { title: "Success" });
    } catch (err) {
      console.error(err);
      await message("Video capture started successfully!", { title: "Success" });
    }
  }

  // ✅ Start capture
  async function onStart() {
    // if (!outputDir) {
    //   alert("Enter output folder (e.g. D:\\TauriCaptures)");
    //   return;
    // }
    try {
      const msg = await invoke("start_capture", {
        // outputDir: outputDir,
        intervalSecs: Number(intervalSec),
      });
      alert(msg);
      setStatus(await invoke("capture_status"));
    } catch (err) {
      console.error(err);
      alert("Failed to start capture: " + err.message);
    }
  }

  // Stop capture
  async function onStop() {
  try {
    await invoke("stop_capture");
  } catch (e) { console.warn(e); }

  try {
    await invoke("stop_video_capture");
  } catch (e) { console.warn(e); }

  setStatus(await invoke("capture_status"));
  await message("All captures stopped", { title: "Stopped" });
}


  // Periodically fetch activity (every 3s)
  useEffect(() => {
    if (!loggedIn) return;
    const timer = setInterval(async () => {
      try {
        const data = await invoke("get_recent_activity", { limit: 10 });
        if (Array.isArray(data) && data.length > 0) {
          setActivity(data.reverse());
          // Parse the latest activity to get metrics
          try {
            const latest = JSON.parse(data[data.length - 1]);
            setLatestMetrics(latest.metrics);
          } catch (e) {
            console.error("Failed to parse metrics:", e);
          }
        }
        const idle = await invoke("is_idle", { thresholdSecs: 15 });
        setIsIdle(idle);
      } catch (err) {
        console.error("Activity fetch error:", err);
      }
    }, 3000);
    return () => clearInterval(timer);
  }, [loggedIn]);

  // Clear recent activity
  async function clearActivity() {
    try {
      await invoke("clear_activity");
      setActivity([]);
      setLatestMetrics(null);
    } catch (err) {
      console.error(err);
    }
  }

  return (
    <div className="min-h-screen flex flex-col items-center justify-center bg-slate-100 p-6">
      <div className="max-w-4xl w-full bg-white p-6 rounded-xl shadow-lg border">
        <h2 className="text-2xl font-bold text-center mb-4 text-gray-700">
          Spectosoft
        </h2>

        {/* ====================== LOGIN ======================= */}
        {!loggedIn ? (
          <div className="space-y-3 max-w-md mx-auto">
            <input
              placeholder="Username"
              value={user.username}
              onChange={(e) => setUser({ ...user, username: e.target.value })}
              className="w-full border p-2 rounded"
            />
            <input
              type="password"
              placeholder="Password"
              value={user.password}
              onChange={(e) => setUser({ ...user, password: e.target.value })}
              className="w-full border p-2 rounded"
            />
            <button
              onClick={onLogin}
              className="w-full bg-blue-600 text-white py-2 rounded hover:bg-blue-700"
            >
              Login
            </button>
          </div>
        ) : (
          <>
            {/* ====================== SETTINGS ======================= */}
            <div className="mb-2">
              {/* <input
                placeholder="Output dir (e.g. D:\\TauriCaptures)"
                value={outputDir}
                onChange={(e) => setOutputDir(e.target.value)}
                className="w-full border p-2 rounded"
              /> */}
            </div>
            <div className="mb-2">
              <input
                type="number"
                value={intervalSec}
                onChange={(e) => setIntervalSec(e.target.value)}
                className="w-full border p-2 rounded"
                placeholder="Interval (seconds)"
              />
            </div>

            {/* ====================== START / STOP ======================= */}
            <div className="flex gap-3 mb-3">
              <button
                onClick={onStart}
                disabled={status}
                className={`flex-1 py-2 rounded ${
                  status
                    ? "bg-gray-300 text-gray-600"
                    : "bg-green-600 text-white hover:bg-green-700"
                }`}
              >
                Start Screenshot Capture
              </button>
              <button
                onClick={onStartVideoCapture}
                disabled={status}
                className={`flex-1 py-2 rounded ${
                  status
                    ? "bg-gray-300 text-gray-600"
                    : "bg-blue-600 text-white hover:bg-blue-700"
                }`}
              >
                Start Video Capture
              </button>
              <button
                onClick={onStop}
                disabled={!status}
                className={`flex-1 py-2 rounded ${
                  !status
                    ? "bg-gray-300 text-gray-600"
                    : "bg-red-600 text-white hover:bg-red-700"
                }`}
              >
                Stop Capture
              </button>
            </div>

            {/* ====================== STATUS LABELS ======================= */}
            <div className="flex justify-between text-sm mb-4">
              <span
                className={`px-2 py-1 rounded ${
                  status
                    ? "bg-green-100 text-green-700"
                    : "bg-red-100 text-red-700"
                }`}
              >
                {status ? "🟢 Capturing" : "🔴 Stopped"}
              </span>
              <span
                className={`px-2 py-1 rounded ${
                  isIdle
                    ? "bg-yellow-100 text-yellow-700"
                    : "bg-blue-100 text-blue-700"
                }`}
              >
                {isIdle ? "😴 Idle" : "⚡ Active"}
              </span>
            </div>

            {/* ====================== METRICS DASHBOARD ======================= */}
            {latestMetrics && (
              <div className="border rounded-lg p-4 mb-4 bg-gradient-to-br from-blue-50 to-purple-50">
                <h3 className="font-bold text-lg mb-3 text-gray-700">📊 Live Metrics</h3>
                
                <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
                  {/* Keyboard Metrics */}
                  <div className="bg-white p-3 rounded-lg shadow-sm">
                    <div className="text-xs text-gray-500 mb-1">⌨️ Keyboard</div>
                    <div className="text-sm space-y-1">
                      <div>Chars: <span className="font-bold">{latestMetrics.char_count}</span></div>
                      <div>Enter: <span className="font-bold">{latestMetrics.enter_count}</span></div>
                      <div>Backspace: <span className="font-bold">{latestMetrics.backspace_count}</span></div>
                    </div>
                  </div>

                  {/* Mouse Clicks */}
                  <div className="bg-white p-3 rounded-lg shadow-sm">
                    <div className="text-xs text-gray-500 mb-1">🖱️ Mouse Clicks</div>
                    <div className="text-sm space-y-1">
                      <div>Left: <span className="font-bold text-blue-600">{latestMetrics.mouse.left_clicks}</span></div>
                      <div>Right: <span className="font-bold text-green-600">{latestMetrics.mouse.right_clicks}</span></div>
                      <div>Middle: <span className="font-bold text-purple-600">{latestMetrics.mouse.middle_clicks}</span></div>
                    </div>
                  </div>

                  {/* Mouse Actions */}
                  <div className="bg-white p-3 rounded-lg shadow-sm">
                    <div className="text-xs text-gray-500 mb-1">🎯 Mouse Actions</div>
                    <div className="text-sm space-y-1">
                      <div>Moves: <span className="font-bold">{latestMetrics.mouse.moves}</span></div>
                      <div>Drags: <span className="font-bold text-orange-600">{latestMetrics.mouse.drags}</span></div>
                      <div>Scrolls: <span className="font-bold text-indigo-600">{latestMetrics.mouse.scrolls}</span></div>
                    </div>
                  </div>

                  {/* Shortcuts */}
                  <div className="bg-white p-3 rounded-lg shadow-sm">
                    <div className="text-xs text-gray-500 mb-1">⚡ Shortcuts</div>
                    <div className="text-sm space-y-1">
                      <div>Copy: <span className="font-bold">{latestMetrics.copy_count}</span></div>
                      <div>Paste: <span className="font-bold">{latestMetrics.paste_count}</span></div>
                      <div>Ctrl: <span className="font-bold">{latestMetrics.mods.ctrl}</span></div>
                    </div>
                  </div>
                </div>

                {/* Additional Stats */}
                <div className="grid grid-cols-2 gap-4 mt-4">
                  <div className="bg-white p-3 rounded-lg shadow-sm">
                    <div className="text-xs text-gray-500 mb-1">🧭 Navigation</div>
                    <div className="text-sm space-y-1">
                      <div>Arrows: <span className="font-bold">{latestMetrics.nav_keys.arrows}</span></div>
                      <div>Pg Up/Dn: <span className="font-bold">{latestMetrics.nav_keys.pgup_pgdn}</span></div>
                      <div>Home/End: <span className="font-bold">{latestMetrics.nav_keys.home_end}</span></div>
                    </div>
                  </div>

                  <div className="bg-white p-3 rounded-lg shadow-sm">
                    <div className="text-xs text-gray-500 mb-1">🎛️ Modifiers</div>
                    <div className="text-sm space-y-1">
                      <div>Alt: <span className="font-bold">{latestMetrics.mods.alt}</span></div>
                      <div>Shift: <span className="font-bold">{latestMetrics.mods.shift}</span></div>
                      <div>F-Keys: <span className="font-bold">{latestMetrics.function_keys.f1_f12}</span></div>
                    </div>
                  </div>
                </div>
              </div>
            )}

            {/* ====================== ACTIVITY LOG ======================= */}
            <div className="border-t pt-4">
              <div className="flex justify-between mb-2">
                <strong className="text-gray-700">📝 Raw Activity Log</strong>
                <button
                  onClick={clearActivity}
                  className="text-red-500 text-sm hover:underline"
                >
                  Clear
                </button>
              </div>
              <div className="h-40 overflow-y-auto bg-gray-50 border rounded p-3 font-mono text-xs">
                {activity.length ? (
                  activity.map((a, i) => {
                    try {
                      const parsed = JSON.parse(a);
                      return (
                        <div key={i} className="mb-2 pb-2 border-b border-gray-200 last:border-0">
                          <div className="text-blue-600 font-semibold">
                            {parsed.app_name} - {parsed.window_title.substring(0, 50)}...
                          </div>
                          <div className="text-gray-500 text-xs">
                            {parsed.timestamp} | PID: {parsed.pid}
                          </div>
                        </div>
                      );
                    } catch (e) {
                      return (
                        <div key={i} className="truncate text-gray-600">
                          {typeof a === "string" ? a : JSON.stringify(a)}
                        </div>
                      );
                    }
                  })
                ) : (
                  <p className="text-gray-400 text-center py-8">No activity yet</p>
                )}
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  );
}