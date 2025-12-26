use futures_util::{SinkExt, StreamExt};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

const DEFAULT_HTTP_PORT: usize = 8080;
const DEFAULT_EVENT_PORT: usize = 9777;

const DEFAULT_WS_PORT: usize = 9090;

fn default_http_port() -> usize {
    DEFAULT_HTTP_PORT
}

fn default_event_port() -> usize {
    DEFAULT_EVENT_PORT
}

fn truthy() -> bool {
    true
}
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Instance {
    label: String,

    hostname: String,

    #[serde(default = "default_http_port")]
    http_port: usize,

    username: Option<String>,

    password: Option<String>,

    #[serde(default = "truthy")]
    events: bool,

    #[serde(default = "default_event_port")]
    events_port: usize,
}

impl Instance {
    fn send_key(&self, key: &str) -> Result<(), String> {
        // Build the JSON-RPC request
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "Input.ExecuteAction",
            "params": {
                "action": key
            },
            "id": 1
        });

        // Build the URL
        let url = format!("http://{}:{}/jsonrpc", self.hostname, self.http_port);

        // Create the HTTP client
        let client = reqwest::blocking::Client::new();
        let mut req_builder = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request);

        // Add authentication if present
        if let (Some(username), Some(password)) = (&self.username, &self.password) {
            req_builder = req_builder.basic_auth(username, Some(password));
        }

        // Send the request
        let response = req_builder
            .send()
            .map_err(|e| format!("Failed to send request: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("Request failed with status: {}", response.status()));
        }

        Ok(())
    }
}

#[derive(Clone, Default, Debug)]
struct NowPlaying {
    title: String,
    artist: String,
    album: String,
    playing: bool,
    duration: f64, // Total duration in seconds
    position: f64, // Current position in seconds
}

impl NowPlaying {
    fn clear(&mut self) {
        self.title.clear();
        self.artist.clear();
        self.album.clear();
        self.playing = false;
        self.duration = 0.0;
        self.position = 0.0;
    }
}

struct Koco {
    config: KocoConfig,
    current_instance_index: usize,
    show_settings_window: bool,
    settings_tab: SettingsTab,
    config_ui_state: ConfigUIState,
    now_playing: Arc<Mutex<NowPlaying>>,
    runtime: Arc<Runtime>,
}

#[derive(Default, PartialEq)]
enum SettingsTab {
    #[default]
    Edit,
    Add,
}

#[derive(Default)]
struct ConfigUIState {
    // For editing current instance
    label: String,
    hostname: String,
    http_port: String,
    username: String,
    password: String,
    events: bool,
    events_port: String,

    // For adding new instance
    new_label: String,
    new_hostname: String,
    new_http_port: String,
    new_username: String,
    new_password: String,
    new_events: bool,
    new_events_port: String,
}

impl ConfigUIState {
    fn load_from_instance(&mut self, instance: &Instance) {
        self.label = instance.label.clone();
        self.hostname = instance.hostname.clone();
        self.http_port = instance.http_port.to_string();
        self.username = instance.username.clone().unwrap_or_default();
        self.password = instance.password.clone().unwrap_or_default();
        self.events = instance.events;
        self.events_port = instance.events_port.to_string();
    }

    fn to_instance(&self) -> Result<Instance, String> {
        let http_port = self
            .http_port
            .parse::<usize>()
            .map_err(|_| "Invalid HTTP port number")?;
        let events_port = self
            .events_port
            .parse::<usize>()
            .map_err(|_| "Invalid events port number")?;

        Ok(Instance {
            label: self.label.clone(),
            hostname: self.hostname.clone(),
            http_port,
            username: if self.username.is_empty() {
                None
            } else {
                Some(self.username.clone())
            },
            password: if self.password.is_empty() {
                None
            } else {
                Some(self.password.clone())
            },
            events: self.events,
            events_port,
        })
    }

    fn new_instance(&self) -> Result<Instance, String> {
        if self.new_hostname.is_empty() {
            return Err("Hostname cannot be empty".to_string());
        }

        let http_port = self
            .new_http_port
            .parse::<usize>()
            .unwrap_or(DEFAULT_HTTP_PORT);
        let events_port = self
            .new_events_port
            .parse::<usize>()
            .unwrap_or(DEFAULT_EVENT_PORT);

        Ok(Instance {
            label: self.new_label.clone(),
            hostname: self.new_hostname.clone(),
            http_port,
            username: if self.new_username.is_empty() {
                None
            } else {
                Some(self.new_username.clone())
            },
            password: if self.new_password.is_empty() {
                None
            } else {
                Some(self.new_password.clone())
            },
            events: self.new_events,
            events_port,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Theme {
    name: String,
    background_color: [u8; 3],
    text_color: [u8; 3],
    heading_colors: Vec<[u8; 3]>,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            name: "Default".to_string(),
            background_color: [40, 44, 52],
            text_color: [220, 223, 228],
            heading_colors: vec![
                [255, 180, 100], // H1
                [230, 160, 90],  // H2
                [210, 140, 80],  // H3
                [190, 120, 70],  // H4
                [170, 100, 60],  // H5
                [150, 80, 50],   // H6
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KocoConfig {
    instances: Vec<Instance>,
    current_theme: String,
    themes: Vec<Theme>,
}

fn create_default_themes() -> Vec<Theme> {
    vec![
        Theme {
            name: "Light".to_string(),
            background_color: [240, 240, 245],
            text_color: [60, 60, 70],
            heading_colors: vec![
                [100, 100, 180], // H1
                [90, 90, 170],   // H2
                [80, 80, 160],   // H3
                [70, 70, 150],   // H4
                [60, 60, 140],   // H5
                [50, 50, 130],   // H6
            ],
        },
        Theme {
            name: "Dark".to_string(),
            background_color: [40, 44, 52],
            text_color: [220, 223, 228],
            heading_colors: vec![
                [255, 180, 100], // H1
                [230, 160, 90],  // H2
                [210, 140, 80],  // H3
                [190, 120, 70],  // H4
                [170, 100, 60],  // H5
                [150, 80, 50],   // H6
            ],
        },
        Theme {
            name: "Solarized".to_string(),
            background_color: [0, 43, 54],
            text_color: [131, 148, 150],
            heading_colors: vec![
                [181, 137, 0],   // H1
                [203, 75, 22],   // H2
                [220, 50, 47],   // H3
                [211, 54, 130],  // H4
                [108, 113, 196], // H5
                [38, 139, 210],  // H6
            ],
        },
        Theme {
            name: "After Dark".to_string(),
            background_color: [32, 29, 101], // base-100: #201D65
            text_color: [172, 171, 213],     // secondary: #ACABD5
            heading_colors: vec![
                [254, 243, 199], // accent: #fef3c7 - H1
                [123, 121, 181], // primary: #7B79B5 - H2
                [172, 171, 213], // secondary: #ACABD5 - H3
                [125, 211, 252], // info: #7dd3fc - H4
                [167, 243, 208], // success: #a7f3d0 - H5
                [254, 240, 138], // warning: #fef08a - H6
            ],
        },
        Theme {
            name: "Her".to_string(),
            background_color: [101, 29, 29], // base-100: #651d1d
            text_color: [213, 171, 171],     // secondary: #d5abab
            heading_colors: vec![
                [254, 243, 199], // accent: #fef3c7 - H1
                [181, 121, 121], // primary: #b57979 - H2
                [213, 171, 171], // secondary: #d5abab - H3
                [125, 211, 252], // info: #7dd3fc - H4
                [167, 243, 208], // success: #a7f3d0 - H5
                [254, 240, 138], // warning: #fef08a - H6
            ],
        },
        Theme {
            name: "Forest".to_string(),
            background_color: [5, 46, 22], // base-100: #052e16
            text_color: [134, 239, 172],   // secondary: #86efac
            heading_colors: vec![
                [254, 243, 199], // accent: #fef3c7 - H1
                [74, 222, 128],  // primary: #4ade80 - H2
                [134, 239, 172], // secondary: #86efac - H3
                [125, 211, 252], // info: #7dd3fc - H4
                [167, 243, 208], // success: #a7f3d0 - H5
                [254, 240, 138], // warning: #fef08a - H6
            ],
        },
        Theme {
            name: "Sky".to_string(),
            background_color: [8, 47, 73], // base-100: #082f49
            text_color: [125, 211, 252],   // secondary: #7dd3fc
            heading_colors: vec![
                [254, 243, 199], // accent: #fef3c7 - H1
                [56, 189, 248],  // primary: #38bdf8 - H2
                [125, 211, 252], // secondary: #7dd3fc - H3
                [167, 243, 208], // success: #a7f3d0 - H4
                [254, 240, 138], // warning: #fef08a - H5
                [252, 165, 165], // error: #fca5a5 - H6
            ],
        },
        Theme {
            name: "Clays".to_string(),
            background_color: [69, 26, 3], // base-100: #451a03
            text_color: [245, 158, 11],    // secondary: #f59e0b
            heading_colors: vec![
                [254, 243, 199], // accent: #fef3c7 - H1
                [217, 119, 6],   // primary: #d97706 - H2
                [245, 158, 11],  // secondary: #f59e0b - H3
                [125, 211, 252], // info: #7dd3fc - H4
                [167, 243, 208], // success: #a7f3d0 - H5
                [254, 240, 138], // warning: #fef08a - H6
            ],
        },
        Theme {
            name: "Stones".to_string(),
            background_color: [41, 37, 36], // base-100: #292524
            text_color: [156, 163, 175],    // secondary: #9ca3af
            heading_colors: vec![
                [254, 243, 199], // accent: #fef3c7 - H1
                [107, 114, 128], // primary: #6b7280 - H2
                [156, 163, 175], // secondary: #9ca3af - H3
                [125, 211, 252], // info: #7dd3fc - H4
                [167, 243, 208], // success: #a7f3d0 - H5
                [254, 240, 138], // warning: #fef08a - H6
            ],
        },
    ]
}

impl Default for KocoConfig {
    fn default() -> Self {
        KocoConfig {
            instances: vec![Instance {
                label: "LocalBoy".to_owned(),
                hostname: "localhost".to_owned(),
                http_port: DEFAULT_HTTP_PORT,
                username: None,
                password: None,
                events: true,
                events_port: DEFAULT_EVENT_PORT,
            }],
            current_theme: "Default".to_string(),
            themes: create_default_themes(),
        }
    }
}

impl KocoConfig {
    fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("koco");

        fs::create_dir_all(&config_dir).ok();
        config_dir.join("config.toml")
    }

    fn load() -> Self {
        let path = Self::config_path();

        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(contents) => match toml::from_str(&contents) {
                    Ok(config) => {
                        info!("Loaded config from {:?}", path);
                        return config;
                    }
                    Err(e) => {
                        info!("Failed to parse config: {}. Using default.", e);
                    }
                },
                Err(e) => {
                    info!("Failed to read config: {}. Using default.", e);
                }
            }
        }

        Self::default()
    }

    fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        let toml_string = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        let mut file =
            fs::File::create(&path).map_err(|e| format!("Failed to create config file: {}", e))?;

        file.write_all(toml_string.as_bytes())
            .map_err(|e| format!("Failed to write config file: {}", e))?;

        info!("Saved config to {:?}", path);
        Ok(())
    }

    fn get_current_theme(&self) -> Option<&Theme> {
        self.themes.iter().find(|t| t.name == self.current_theme)
    }

    fn get_current_theme_or_default(&self) -> Theme {
        self.get_current_theme().cloned().unwrap_or_default()
    }
}

impl Koco {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Initialize material icons
        egui_material_icons::initialize(&cc.egui_ctx);

        let config = KocoConfig::load();

        // Apply theme
        let theme = config.get_current_theme_or_default();
        let mut visuals = egui::Visuals::dark();

        // Set background color
        visuals.panel_fill = egui::Color32::from_rgb(
            theme.background_color[0],
            theme.background_color[1],
            theme.background_color[2],
        );

        // Set text color
        visuals.override_text_color = Some(egui::Color32::from_rgb(
            theme.text_color[0],
            theme.text_color[1],
            theme.text_color[2],
        ));

        cc.egui_ctx.set_visuals(visuals);

        let mut config_ui_state = ConfigUIState::default();

        // Initialize UI state with first instance
        if !config.instances.is_empty() {
            config_ui_state.load_from_instance(&config.instances[0]);
        }

        let now_playing = Arc::new(Mutex::new(NowPlaying::default()));
        let runtime = Arc::new(Runtime::new().expect("Failed to create Tokio runtime"));

        let app = Self {
            config,
            current_instance_index: 0,
            show_settings_window: false,
            settings_tab: SettingsTab::default(),
            config_ui_state,
            now_playing,
            runtime,
        };

        // Connect to websocket on startup
        app.connect_websocket();

        // Start position updater
        app.start_position_updater();

        app
    }

    fn connect_websocket(&self) {
        if let Some(instance) = self.current_instance() {
            if !instance.events {
                return;
            }

            let ws_url = format!("ws://{}:{}/jsonrpc", instance.hostname, DEFAULT_WS_PORT);
            let now_playing = Arc::clone(&self.now_playing);
            let current_player_id = Arc::new(Mutex::new(None::<i64>));
            let _username = instance.username.clone();
            let _password = instance.password.clone();

            self.runtime.spawn(async move {
                match connect_async(&ws_url).await {
                    Ok((mut ws_stream, _)) => {
                        debug!("Connected to Kodi WebSocket at {}", ws_url);

                        // Subscribe to player notifications
                        // Note: Kodi doesn't require explicit subscription, it broadcasts all notifications

                        // Request initial player state
                        let get_active_players_msg = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "Player.GetActivePlayers",
                            "id": 1
                        });

                        debug!("Requesting active players...");
                        if let Err(e) = ws_stream.send(Message::Text(get_active_players_msg.to_string())).await {
                            debug!("Failed to get active players: {}", e);
                            return;
                        }

                        // Listen for messages
                        while let Some(msg) = ws_stream.next().await {
                            match msg {
                                Ok(Message::Text(text)) => {
                                    debug!("Received WebSocket message: {}", text);
                                    if let Ok(json) = serde_json::from_str::<Value>(&text) {
                                        if let Some(method) = json.get("method").and_then(|m| m.as_str()) {
                                            debug!("Method: {}", method);
                                            match method {
                                                "Player.OnPlay" | "Player.OnResume" => {
                                                    warn!("Player event detected, requesting item info...");
                                                    // Set playing state to true for play/resume
                                                    if let Ok(mut np) = now_playing.lock() {
                                                        np.playing = true;
                                                    }

                                                    // Get player ID from the notification
                                                    let player_id = json.get("params")
                                                        .and_then(|p| p.get("data"))
                                                        .and_then(|d| d.get("player"))
                                                        .and_then(|p| p.get("playerid"))
                                                        .and_then(|id| id.as_i64())
                                                        .unwrap_or(0);

                                                    if let Ok(mut pid) = current_player_id.lock() {
                                                        *pid = Some(player_id);
                                                    }

                                                    debug!("Using player ID: {}", player_id);

                                                    // Request player properties for position/duration
                                                    let properties_msg = serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "method": "Player.GetProperties",
                                                        "params": {
                                                            "playerid": player_id,
                                                            "properties": ["time", "totaltime", "percentage"]
                                                        },
                                                        "id": 4
                                                    });
                                                    let _ = ws_stream.send(Message::Text(properties_msg.to_string())).await;

                                                    // Request current player info
                                                    let player_info_msg = serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "method": "Player.GetItem",
                                                        "params": {
                                                            "playerid": player_id,
                                                            "properties": ["title", "artist", "album", "duration", "showtitle", "season", "episode"]
                                                        },
                                                        "id": 2
                                                    });
                                                    let _ = ws_stream.send(Message::Text(player_info_msg.to_string())).await;
                                                }
                                                "Player.OnPause" => {
                                                    warn!("Player paused");
                                                    if let Ok(mut np) = now_playing.lock() {
                                                        np.playing = false;
                                                    }
                                                }
                                                "Player.OnStop" => {
                                                    warn!("Player stopped");
                                                    if let Ok(mut np) = now_playing.lock() {
                                                        debug!("Clearing now playing info");
                                                        np.clear();
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }

                                        // Handle errors
                                        if let Some(error) = json.get("error") {
                                            info!("Kodi API error: {:?}", error);
                                            info!("Request ID: {:?}", json.get("id"));
                                        }

                                        if let Some(result) = json.get("result") {
                                            info!("Got result: {:?}", result);
                                            info!("Request ID from response: {:?}", json.get("id"));

                                            // Check if this is a direct properties response (not nested in item)
                                            if let Some(totaltime_obj) = result.get("totaltime") {
                                                debug!("Got totaltime at root level: {:?}", totaltime_obj);
                                                if let Ok(mut np) = now_playing.lock() {
                                                    let hours = totaltime_obj.get("hours").and_then(|h| h.as_f64()).unwrap_or(0.0);
                                                    let minutes = totaltime_obj.get("minutes").and_then(|m| m.as_f64()).unwrap_or(0.0);
                                                    let seconds = totaltime_obj.get("seconds").and_then(|s| s.as_f64()).unwrap_or(0.0);
                                                    np.duration = hours * 3600.0 + minutes * 60.0 + seconds;
                                                    debug!("Updated duration from root properties: {}", np.duration);
                                                }
                                            }

                                            // Parse player item info response
                                            if let Some(item) = result.get("item") {
                                                debug!("Parsing item: {:?}", item);
                                                if let Ok(mut np) = now_playing.lock() {
                                                    let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("");
                                                    let label = item.get("label").and_then(|l| l.as_str()).unwrap_or("");

                                                    // Use label if title is empty
                                                    np.title = if !title.is_empty() { title.to_string() } else { label.to_string() };
                                                    np.artist = item.get("artist").and_then(|a| a.as_array()).and_then(|arr| arr.get(0)).and_then(|a| a.as_str()).unwrap_or("").to_string();
                                                    np.album = item.get("album").and_then(|a| a.as_str()).unwrap_or("").to_string();

                                                    debug!("Parsed - Title: '{}', Artist: '{}', Album: '{}'", np.title, np.artist, np.album);
                                                    np.playing = true;
                                                }
                                                // Don't try to get duration from item - it comes from GetProperties
                                            }

                                            // Handle position from root level
                                            if let Some(position_obj) = result.get("time").or_else(|| result.get("position")) {
                                                debug!("Got time/position at root level: {:?}", position_obj);
                                                if let Ok(mut np) = now_playing.lock() {
                                                    let hours = position_obj.get("hours").and_then(|h| h.as_f64()).unwrap_or(0.0);
                                                    let minutes = position_obj.get("minutes").and_then(|m| m.as_f64()).unwrap_or(0.0);
                                                    let seconds = position_obj.get("seconds").and_then(|s| s.as_f64()).unwrap_or(0.0);
                                                    np.position = hours * 3600.0 + minutes * 60.0 + seconds;
                                                    debug!("Updated position: {} / {}", np.position, np.duration);
                                                    np.playing = true;
                                                }
                                            }

                                            // Log summary after parsing
                                            if result.get("item").is_some() || result.get("totaltime").is_some() || result.get("time").is_some() {
                                                if let Ok(np) = now_playing.lock() {
                                                    warn!("==> Now Playing: {} - {} ({}) - {:.0}s / {:.0}s ({:.1}%)",
                                                        np.title, np.artist, np.album, np.position, np.duration,
                                                        if np.duration > 0.0 { (np.position / np.duration) * 100.0 } else { 0.0 });
                                                }
                                            } else if let Some(players) = result.as_array() {
                                                // Response to GetActivePlayers
                                                debug!("Active players: {:?}", players);
                                                if !players.is_empty() {
                                                    // Get the first active player's info
                                                    if let Some(player_id) = players[0].get("playerid").and_then(|id| id.as_i64()) {
                                                        debug!("Found active player with id: {}", player_id);

                                                        // Store the player ID
                                                        if let Ok(mut pid) = current_player_id.lock() {
                                                            *pid = Some(player_id);
                                                        }

                                                        // Request properties first
                                                        let properties_msg = serde_json::json!({
                                                            "jsonrpc": "2.0",
                                                            "method": "Player.GetProperties",
                                                            "params": {
                                                                "playerid": player_id,
                                                                "properties": ["time", "totaltime", "percentage"]
                                                            },
                                                            "id": 5
                                                        });
                                                        let _ = ws_stream.send(Message::Text(properties_msg.to_string())).await;

                                                        // Then request item info
                                                        let player_info_msg = serde_json::json!({
                                                            "jsonrpc": "2.0",
                                                            "method": "Player.GetItem",
                                                            "params": {
                                                                "playerid": player_id,
                                                                "properties": ["title", "artist", "album", "duration", "showtitle", "season", "episode"]
                                                            },
                                                            "id": 6
                                                        });
                                                        let _ = ws_stream.send(Message::Text(player_info_msg.to_string())).await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                Ok(Message::Close(_)) => {
                                    debug!("WebSocket closed");
                                    break;
                                }
                                Err(e) => {
                                    debug!("WebSocket error: {}", e);
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Failed to connect to WebSocket: {}", e);
                    }
                }
            });
        }
    }

    fn start_position_updater(&self) {
        let now_playing = Arc::clone(&self.now_playing);

        self.runtime.spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(1));
            loop {
                interval.tick().await;
                if let Ok(mut np) = now_playing.lock() {
                    if np.playing && np.position < np.duration {
                        np.position += 1.0;
                    }
                }
            }
        });
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        let theme = self.config.get_current_theme_or_default();
        let mut visuals = egui::Visuals::dark();

        visuals.panel_fill = egui::Color32::from_rgb(
            theme.background_color[0],
            theme.background_color[1],
            theme.background_color[2],
        );

        visuals.override_text_color = Some(egui::Color32::from_rgb(
            theme.heading_colors[0][0],
            theme.heading_colors[0][1],
            theme.heading_colors[0][2],
        ));

        ctx.set_visuals(visuals);
    }

    fn render_settings_window(&mut self, ctx: &egui::Context) {
        let mut theme_changed = false;

        egui::Window::new("Koco Settings")
            .open(&mut self.show_settings_window)
            .resizable(true)
            .default_size([500.0, 600.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(10.0);

                    // Instance selector
                    ui.horizontal(|ui| {
                        ui.label("Instance:");
                        egui::ComboBox::from_id_salt("instance_selector")
                            .selected_text(
                                if self.current_instance_index < self.config.instances.len() {
                                    &self.config.instances[self.current_instance_index].label
                                } else {
                                    "None"
                                },
                            )
                            .show_ui(ui, |ui| {
                                for (i, instance) in self.config.instances.iter().enumerate() {
                                    if ui
                                        .selectable_label(
                                            i == self.current_instance_index,
                                            &instance.hostname,
                                        )
                                        .clicked()
                                    {
                                        self.current_instance_index = i;
                                        self.config_ui_state.load_from_instance(instance);
                                    }
                                }
                            });
                    });

                    ui.separator();

                    // Theme selector
                    ui.horizontal(|ui| {
                        ui.label("Theme:");
                        egui::ComboBox::from_id_salt("theme_selector")
                            .selected_text(&self.config.current_theme)
                            .show_ui(ui, |ui| {
                                for theme in &self.config.themes {
                                    if ui
                                        .selectable_label(
                                            theme.name == self.config.current_theme,
                                            &theme.name,
                                        )
                                        .clicked()
                                    {
                                        self.config.current_theme = theme.name.clone();
                                        theme_changed = true;
                                        let _ = self.config.save();
                                    }
                                }
                            });
                    });

                    ui.separator();

                    // Tabs for Edit/Add
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.settings_tab,
                            SettingsTab::Edit,
                            "Edit Instance",
                        );
                        ui.selectable_value(
                            &mut self.settings_tab,
                            SettingsTab::Add,
                            "Add Instance",
                        );
                    });

                    ui.separator();

                    match self.settings_tab {
                        SettingsTab::Edit => {
                            // Edit current instance
                            if self.current_instance_index < self.config.instances.len() {
                                ui.heading("Edit Instance");

                                ui.horizontal(|ui| {
                                    ui.label("Label:");
                                    ui.text_edit_singleline(&mut self.config_ui_state.label);
                                });

                                ui.horizontal(|ui| {
                                    ui.label("Hostname:");
                                    ui.text_edit_singleline(&mut self.config_ui_state.hostname);
                                });

                                ui.horizontal(|ui| {
                                    ui.label("HTTP Port:");
                                    ui.text_edit_singleline(&mut self.config_ui_state.http_port);
                                });

                                ui.horizontal(|ui| {
                                    ui.label("Username:");
                                    ui.text_edit_singleline(&mut self.config_ui_state.username);
                                });

                                ui.horizontal(|ui| {
                                    ui.label("Password:");
                                    ui.add(
                                        egui::TextEdit::singleline(
                                            &mut self.config_ui_state.password,
                                        )
                                        .password(true),
                                    );
                                });

                                ui.checkbox(&mut self.config_ui_state.events, "Enable Events");

                                if self.config_ui_state.events {
                                    ui.horizontal(|ui| {
                                        ui.label("Events Port:");
                                        ui.text_edit_singleline(
                                            &mut self.config_ui_state.events_port,
                                        );
                                    });
                                }

                                ui.horizontal(|ui| {
                                    if ui.button("Update Instance").clicked() {
                                        match self.config_ui_state.to_instance() {
                                            Ok(instance) => {
                                                self.config.instances
                                                    [self.current_instance_index] = instance;
                                                if let Err(e) = self.config.save() {
                                                    warn!("Failed to save config: {}", e);
                                                }
                                            }
                                            Err(e) => info!("Invalid instance data: {}", e),
                                        }
                                    }

                                    if ui.button("Delete Instance").clicked()
                                        && self.config.instances.len() > 1
                                    {
                                        self.config.instances.remove(self.current_instance_index);
                                        self.current_instance_index =
                                            self.current_instance_index.saturating_sub(1);
                                        if !self.config.instances.is_empty() {
                                            self.config_ui_state.load_from_instance(
                                                &self.config.instances[self.current_instance_index],
                                            );
                                        }
                                        if let Err(e) = self.config.save() {
                                            warn!("Failed to save config: {}", e);
                                        }
                                    }
                                });
                            } else {
                                ui.label("No instance selected");
                            }
                        }
                        SettingsTab::Add => {
                            // Add new instance
                            ui.heading("Add New Instance");

                            ui.horizontal(|ui| {
                                ui.label("Label:");
                                ui.text_edit_singleline(&mut self.config_ui_state.new_label);
                            });

                            ui.horizontal(|ui| {
                                ui.label("Hostname:");
                                ui.text_edit_singleline(&mut self.config_ui_state.new_hostname);
                            });

                            ui.horizontal(|ui| {
                                ui.label("HTTP Port:");
                                ui.text_edit_singleline(&mut self.config_ui_state.new_http_port);
                            });

                            ui.horizontal(|ui| {
                                ui.label("Username:");
                                ui.text_edit_singleline(&mut self.config_ui_state.new_username);
                            });

                            ui.horizontal(|ui| {
                                ui.label("Password:");
                                ui.add(
                                    egui::TextEdit::singleline(
                                        &mut self.config_ui_state.new_password,
                                    )
                                    .password(true),
                                );
                            });

                            ui.checkbox(&mut self.config_ui_state.new_events, "Enable Events");

                            if self.config_ui_state.new_events {
                                ui.horizontal(|ui| {
                                    ui.label("Events Port:");
                                    ui.text_edit_singleline(
                                        &mut self.config_ui_state.new_events_port,
                                    );
                                });
                            }

                            if ui.button("Add Instance").clicked() {
                                match self.config_ui_state.new_instance() {
                                    Ok(instance) => {
                                        self.config.instances.push(instance);
                                        // Clear new instance fields
                                        self.config_ui_state.new_label.clear();
                                        self.config_ui_state.new_hostname.clear();
                                        self.config_ui_state.new_http_port.clear();
                                        self.config_ui_state.new_username.clear();
                                        self.config_ui_state.new_password.clear();
                                        self.config_ui_state.new_events = true;
                                        self.config_ui_state.new_events_port.clear();

                                        if let Err(e) = self.config.save() {
                                            warn!("Failed to save config: {}", e);
                                        }
                                    }
                                    Err(e) => warn!("Failed to add instance: {}", e),
                                }
                            }
                        }
                    }

                    ui.separator();
                    ui.label(format!("Config file: {:?}", KocoConfig::config_path()));
                    ui.add_space(10.0);
                });
            });

        // Apply theme outside the closure to avoid borrow checker issues
        if theme_changed {
            self.apply_theme(ctx);
        }
    }

    fn current_instance(&self) -> Option<&Instance> {
        self.config.instances.get(self.current_instance_index)
    }
}

impl eframe::App for Koco {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Update window title with now-playing info
        let window_title = if let Ok(np) = self.now_playing.lock() {
            if !np.title.is_empty() {
                let status_icon = if np.playing { "▶" } else { "⏸" };
                if !np.artist.is_empty() {
                    format!("{} {} - {} - Koco", status_icon, np.title, np.artist)
                } else {
                    format!("{} {} - Koco", status_icon, np.title)
                }
            } else {
                "Koco - Kodi Control".to_string()
            }
        } else {
            "Koco - Kodi Control".to_string()
        };

        ctx.send_viewport_cmd(egui::ViewportCommand::Title(window_title));

        // Handle keyboard input
        if let Some(instance) = self.current_instance() {
            ctx.input(|i| {
                if i.key_pressed(egui::Key::ArrowUp) {
                    if let Err(e) = instance.send_key("up") {
                        info!("Failed to send up command: {}", e);
                    }
                }
                if i.key_pressed(egui::Key::ArrowDown) {
                    if let Err(e) = instance.send_key("down") {
                        info!("Failed to send down command: {}", e);
                    }
                }
                if i.key_pressed(egui::Key::ArrowLeft) {
                    if let Err(e) = instance.send_key("left") {
                        info!("Failed to send left command: {}", e);
                    }
                }
                if i.key_pressed(egui::Key::ArrowRight) {
                    if let Err(e) = instance.send_key("right") {
                        info!("Failed to send right command: {}", e);
                    }
                }
                if i.key_pressed(egui::Key::Enter) {
                    if let Err(e) = instance.send_key("select") {
                        info!("Failed to send select command: {}", e);
                    }
                }
                if i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Backspace) {
                    if let Err(e) = instance.send_key("back") {
                        info!("Failed to send back command: {}", e);
                    }
                }
                if i.key_pressed(egui::Key::Space) {
                    if let Err(e) = instance.send_key("playpause") {
                        info!("Failed to send playpause command: {}", e);
                    }
                }
            });
        }

        let label = self
            .current_instance()
            .map(|i| i.label.clone())
            .unwrap_or_else(|| "No instance selected".to_string());

        // Top menu bar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!("Koco - {}", label));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(
                            egui::RichText::new(egui_material_icons::icons::ICON_SETTINGS)
                                .size(20.0),
                        )
                        .clicked()
                    {
                        self.show_settings_window = true;
                    }
                });
            });
        });

        // Show settings window
        if self.show_settings_window {
            self.render_settings_window(ctx);
        }

        // Main panel
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(instance) = self.current_instance() {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);

                    // Grid for the DPAD and Select button with sized buttons
                    egui::Grid::new("remote_grid")
                        .spacing([10.0, 10.0])
                        .show(ui, |ui| {
                            // Row 1: Empty, Up, Empty
                            ui.label(""); // Empty cell
                            if ui
                                .add_sized(
                                    [80.0, 80.0],
                                    egui::Button::new(
                                        egui::RichText::new(
                                            egui_material_icons::icons::ICON_ARROW_UPWARD,
                                        )
                                        .size(48.0),
                                    ),
                                )
                                .clicked()
                            {
                                if let Err(e) = instance.send_key("up") {
                                    info!("Failed to send up command: {}", e);
                                }
                            }
                            ui.label(""); // Empty cell
                            ui.end_row();

                            // Row 2: Left, Select, Right
                            if ui
                                .add_sized(
                                    [80.0, 80.0],
                                    egui::Button::new(
                                        egui::RichText::new(
                                            egui_material_icons::icons::ICON_ARROW_BACK,
                                        )
                                        .size(48.0),
                                    ),
                                )
                                .clicked()
                            {
                                if let Err(e) = instance.send_key("left") {
                                    info!("Failed to send left command: {}", e);
                                }
                            }
                            if ui
                                .add_sized(
                                    [80.0, 80.0],
                                    egui::Button::new(
                                        egui::RichText::new(
                                            egui_material_icons::icons::ICON_RADIO_BUTTON_CHECKED,
                                        )
                                        .size(48.0),
                                    ),
                                )
                                .clicked()
                            {
                                if let Err(e) = instance.send_key("select") {
                                    info!("Failed to send select command: {}", e);
                                }
                            }
                            if ui
                                .add_sized(
                                    [80.0, 80.0],
                                    egui::Button::new(
                                        egui::RichText::new(
                                            egui_material_icons::icons::ICON_ARROW_FORWARD,
                                        )
                                        .size(48.0),
                                    ),
                                )
                                .clicked()
                            {
                                if let Err(e) = instance.send_key("right") {
                                    info!("Failed to send right command: {}", e);
                                }
                            }
                            ui.end_row();

                            // Row 3: Empty, Down, Empty
                            if ui
                                .add_sized(
                                    [80.0, 80.0],
                                    egui::Button::new(
                                        egui::RichText::new(
                                            egui_material_icons::icons::ICON_REPLAY,
                                        )
                                        .size(48.0),
                                    ),
                                )
                                .clicked()
                            {
                                if let Err(e) = instance.send_key("back") {
                                    info!("Failed to send back command: {}", e);
                                }
                            }
                            if ui
                                .add_sized(
                                    [80.0, 80.0],
                                    egui::Button::new(
                                        egui::RichText::new(
                                            egui_material_icons::icons::ICON_ARROW_DOWNWARD,
                                        )
                                        .size(48.0),
                                    ),
                                )
                                .clicked()
                            {
                                if let Err(e) = instance.send_key("down") {
                                    info!("Failed to send down command: {}", e);
                                }
                            }
                            ui.label(""); // Empty cell
                            ui.end_row();
                        });

                    ui.add_space(20.0);
                });
            } else {
                ui.vertical_centered(|ui| {
                    ui.heading("No instances configured");
                    ui.label("Click the settings icon to add an instance");
                });
            }
        });

        // Bottom panel for now playing
        egui::TopBottomPanel::bottom("now_playing_panel").show(ctx, |ui| {
            if let Ok(np) = self.now_playing.lock() {
                if !np.title.is_empty() {
                    // Calculate progress percentage
                    let percentage = if np.duration > 0.0 {
                        (np.position / np.duration).min(1.0).max(0.0)
                    } else {
                        0.0
                    };

                    // Draw custom background with progress
                    let available_rect = ui.available_rect_before_wrap();
                    let progress_width = available_rect.width() * percentage as f32;

                    // Draw progress background
                    let progress_rect = egui::Rect::from_min_size(
                        available_rect.min,
                        egui::vec2(progress_width, available_rect.height()),
                    );
                    ui.painter().rect_filled(
                        progress_rect,
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(100, 100, 255, 50),
                    );

                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        let status_icon = if np.playing { "▶" } else { "⏸" };
                        ui.label(egui::RichText::new(status_icon).size(16.0));
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&np.title).size(16.0).strong());
                            if !np.artist.is_empty() {
                                ui.label(egui::RichText::new(&np.artist).size(12.0).weak());
                            }
                            if !np.album.is_empty() {
                                ui.label(egui::RichText::new(&np.album).size(11.0).weak());
                            }
                        });
                    });
                } else {
                    ui.label("No media playing");
                }
            }
            ui.add_space(5.0);
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([280.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Koco - Kodi Control",
        options,
        Box::new(|cc| Ok(Box::new(Koco::new(cc)))),
    )
}
