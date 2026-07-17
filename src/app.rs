use std::sync::{Arc, Mutex};

use anyhow::Result;
use crossbeam_channel::bounded;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::asr::{AsrWorker, ModelKind, ModelRegistry, ModelSelection};
use crate::audio::{AudioCommand, AudioEvent, AudioWorker, list_input_devices};
use crate::commands::AsrCommand;
use crate::config::SettingsStore;
use crate::events::AsrEvent;
use crate::hotkey::{HotkeyAction, HotkeyBinding, HotkeyStateMachine, KeyboardHook, vk_to_display_name};
use crate::output::{OutputCommand, OutputEvent, OutputMode, OutputWorker};
use crate::overlay::{OverlayCommand, OverlayWindow};
use crate::paths::AppPaths;
use crate::state::SessionState;
use crate::tray::{AppTray, TrayCommand};
use crate::ui::AppWindow;

enum HotkeyControl {
    UpdateBinding(HotkeyBinding),
    StartCapture,
    CancelCapture,
}

pub fn run() -> Result<()> {
    let settings_store = SettingsStore::discover()?;
    let settings = settings_store.load_or_default()?;

    let (command_sender, command_receiver) = bounded(32);
    let (asr_audio_sender, asr_audio_receiver) = bounded(256);
    let (event_sender, event_receiver) = bounded(32);
    let worker = AsrWorker::spawn(command_receiver, asr_audio_receiver, event_sender);
    let (audio_command_sender, audio_command_receiver) = bounded(16);
    let (audio_event_sender, audio_event_receiver) = bounded(32);
    let audio_worker =
        AudioWorker::spawn(audio_command_receiver, audio_event_sender, asr_audio_sender);
    if !settings.audio.device_name.is_empty() {
        let _ = audio_command_sender.send(AudioCommand::SelectDevice {
            name: settings.audio.device_name.clone(),
        });
    }
    let (output_command_sender, output_command_receiver) = bounded(16);
    let (output_event_sender, output_event_receiver) = bounded(16);
    let output_worker = OutputWorker::spawn(
        output_command_receiver,
        output_event_sender,
        OutputMode::parse(&settings.output.mode),
        settings.output.strip_trailing_punctuation,
    );
    let overlay = OverlayWindow::spawn()?;
    let overlay_sender = overlay.sender();

    let state = Arc::new(Mutex::new(SessionState::default()));
    let active_model = Arc::new(Mutex::new(settings.asr.model));
    let model_test_session = Arc::new(Mutex::new(None));
    let window = AppWindow::new()?;
    let app_paths = AppPaths::discover()?;
    app_paths.ensure_directories()?;
    window.set_status_text("正在启动".into());
    window.set_model_index(match settings.asr.model {
        crate::asr::ModelKind::SenseVoice => 0,
        crate::asr::ModelKind::FunAsrNano => 1,
        crate::asr::ModelKind::Qwen3Asr => 2,
    });
    window.set_model_name(match settings.asr.model {
        crate::asr::ModelKind::SenseVoice => "SenseVoice-Small".into(),
        crate::asr::ModelKind::FunAsrNano => "Fun-ASR-Nano".into(),
        crate::asr::ModelKind::Qwen3Asr => "Qwen3-ASR".into(),
    });
    window.set_model_capability_text(model_capability_text(settings.asr.model).into());
    window.set_hotkey_text(settings.hotkey.keys.join(" + ").into());
    window.set_hotwords_text(settings.hotwords.items.join("\n").into());
    window.set_suppress_hotkey(false);
    window.set_output_mode_index(match settings.output.mode.as_str() {
        "clipboard" | "paste" => 1,
        "copy" | "copy_only" => 2,
        _ => 0,
    });
    window.set_strip_trailing_punctuation(settings.output.strip_trailing_punctuation);
    let microphone_devices = list_input_devices().unwrap_or_default();
    let microphone_names = microphone_devices
        .iter()
        .map(|device| SharedString::from(device.name.as_str()))
        .collect::<Vec<_>>();
    let microphone_index = microphone_devices
        .iter()
        .position(|device| {
            (!settings.audio.device_name.is_empty() && device.name == settings.audio.device_name)
                || (settings.audio.device_name.is_empty() && device.is_default)
        })
        .unwrap_or(0);
    window.set_microphone_devices(ModelRc::new(VecModel::from(microphone_names)));
    window.set_microphone_index(microphone_index as i32);
    let models_directory = app_paths.models.clone();
    window.on_open_models_directory(move || {
        let _ = std::process::Command::new("explorer.exe")
            .arg(&models_directory)
            .spawn();
    });
    let logs_directory = app_paths.logs.clone();
    window.on_open_logs_directory(move || {
        let _ = std::process::Command::new("explorer.exe")
            .arg(&logs_directory)
            .spawn();
    });
    let diagnostics_root = app_paths.root.clone();
    let diagnostics_microphones = microphone_devices.clone();
    let weak_window_for_diagnostics = window.as_weak();
    window.on_copy_diagnostics(move || {
        let Some(window) = weak_window_for_diagnostics.upgrade() else {
            return;
        };
        let microphone = diagnostics_microphones
            .get(window.get_microphone_index() as usize)
            .map(|device| device.name.as_str())
            .unwrap_or("Unavailable");
        let summary = format!(
            "DictateIn {}\n状态：{}\n模型：{}\n麦克风：{}\n程序目录：{}\n",
            env!("CARGO_PKG_VERSION"),
            window.get_status_text(),
            window.get_model_name(),
            microphone,
            diagnostics_root.display(),
        );
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(summary)) {
            Ok(()) => window.set_status_text("诊断摘要已复制".into()),
            Err(error) => window.set_status_text(error.to_string().into()),
        }
    });
    let selected_microphone = Arc::new(Mutex::new(
        microphone_devices
            .get(microphone_index)
            .map(|device| (device.id.clone(), device.name.clone())),
    ));
    let devices_for_microphone = microphone_devices.clone();
    let audio_for_microphone = audio_command_sender.clone();
    let selected_for_microphone = Arc::clone(&selected_microphone);
    window.on_select_microphone(move |index| {
        let Some(device) = devices_for_microphone.get(index as usize).cloned() else {
            return;
        };
        if let Ok(mut selected) = selected_for_microphone.lock() {
            *selected = Some((device.id, device.name.clone()));
        }
        let _ = audio_for_microphone.send(AudioCommand::SelectDevice { name: device.name });
    });

    let (tray_command_sender, tray_command_receiver) = bounded(8);
    let _tray = AppTray::create(tray_command_sender)?;
    let weak_window_for_tray = window.as_weak();
    std::thread::Builder::new()
        .name("tray-command-bridge".into())
        .spawn(move || {
            while let Ok(command) = tray_command_receiver.recv() {
                let weak_window = weak_window_for_tray.clone();
                let _ = slint::invoke_from_event_loop(move || match command {
                    TrayCommand::ShowSettings => {
                        if let Some(window) = weak_window.upgrade() {
                            let _ = window.show();
                        }
                    }
                    TrayCommand::Exit => {
                        let _ = slint::quit_event_loop();
                    }
                });
            }
        })?;

    window
        .window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);

    let weak_window = window.as_weak();
    let state_for_events = Arc::clone(&state);
    let output_for_events = output_command_sender.clone();
    let overlay_for_events = overlay_sender.clone();
    let audio_for_events = audio_command_sender.clone();
    let active_model_for_events = Arc::clone(&active_model);
    let model_test_for_events = Arc::clone(&model_test_session);
    std::thread::spawn(move || {
        while let Ok(event) = event_receiver.recv() {
            let weak_window = weak_window.clone();
            let state = Arc::clone(&state_for_events);
            let output = output_for_events.clone();
            let overlay = overlay_for_events.clone();
            let audio = audio_for_events.clone();
            let active_model = Arc::clone(&active_model_for_events);
            let model_test = Arc::clone(&model_test_for_events);
            let _ = slint::invoke_from_event_loop(move || {
                handle_asr_event(
                    AsrEventContext {
                        window: &weak_window,
                        state: &state,
                        active_model: &active_model,
                        model_test_session: &model_test,
                        output: &output,
                        overlay: &overlay,
                        audio: &audio,
                    },
                    event,
                );
            });
        }
    });

    let weak_window_for_output = window.as_weak();
    let state_for_output = Arc::clone(&state);
    let overlay_for_output = overlay_sender.clone();
    std::thread::spawn(move || {
        while let Ok(event) = output_event_receiver.recv() {
            let weak_window = weak_window_for_output.clone();
            let state = Arc::clone(&state_for_output);
            let overlay = overlay_for_output.clone();
            let _ = slint::invoke_from_event_loop(move || {
                handle_output_event(&weak_window, &state, &overlay, event);
            });
        }
    });

    let weak_window_for_audio = window.as_weak();
    let state_for_audio = Arc::clone(&state);
    let model_test_for_audio = Arc::clone(&model_test_session);
    let asr_for_audio = command_sender.clone();
    std::thread::spawn(move || {
        while let Ok(event) = audio_event_receiver.recv() {
            match event {
                AudioEvent::Stopped { session_id } => {
                    let should_finish = state_for_audio
                        .lock()
                        .map(|state| {
                            state.accepts(session_id)
                                && state.app_state == crate::state::AppState::Finalizing
                        })
                        .unwrap_or(false);
                    if should_finish {
                        let _ = asr_for_audio.send(AsrCommand::FinishSession { session_id });
                    }
                }
                AudioEvent::Ready {
                    device_name,
                    sample_rate,
                } => {
                    tracing::info!(
                        %device_name,
                        sample_rate,
                        "microphone ready"
                    );
                }
                AudioEvent::Level { session_id, peak } => {
                    let accepts = state_for_audio
                        .lock()
                        .map(|state| state.accepts(session_id))
                        .unwrap_or(false);
                    if accepts {
                        let weak_window = weak_window_for_audio.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(window) = weak_window.upgrade() {
                                window.set_input_level(peak.clamp(0.0, 1.0));
                            }
                        });
                    }
                }
                AudioEvent::Error {
                    session_id,
                    message,
                } => {
                    tracing::error!(?session_id, %message, "audio error");
                    if let Some(session_id) = session_id {
                        let _ = asr_for_audio.send(AsrCommand::CancelSession { session_id });
                        if let Ok(mut state) = state_for_audio.lock() {
                            state.mark_error();
                        }
                        if let Ok(mut test_session) = model_test_for_audio.lock()
                            && *test_session == Some(session_id)
                        {
                            *test_session = None;
                            let weak_window = weak_window_for_audio.clone();
                            let result = message.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(window) = weak_window.upgrade() {
                                    window.set_model_test_state(0);
                                    window.set_model_test_result(result.into());
                                }
                            });
                        }
                    }
                    let weak_window = weak_window_for_audio.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(window) = weak_window.upgrade() {
                            window.set_status_text(message.into());
                        }
                    });
                }
            }
        }
    });

    // Determine which model to load on startup.
    // Prefer the saved model if its files are present; otherwise pick the
    // first model whose files are actually available locally so the app
    // doesn't show a spurious "model not found" error when the user has
    // only downloaded a different model.
    let startup_model = {
        let registry = ModelRegistry::discover().ok();
        let saved = settings.asr.model;
        let all_kinds = [ModelKind::SenseVoice, ModelKind::FunAsrNano, ModelKind::Qwen3Asr];
        let available = |kind| {
            registry
                .as_ref()
                .and_then(|r| r.validate(kind).ok())
                .is_some()
        };
        if available(saved) {
            saved
        } else {
            all_kinds
                .iter()
                .copied()
                .find(|k| available(*k))
                .unwrap_or(saved)
        }
    };
    if startup_model != settings.asr.model {
        tracing::info!(
            saved = ?settings.asr.model,
            actual = ?startup_model,
            "saved model not found locally, loading available model instead"
        );
        window.set_model_index(match startup_model {
            ModelKind::SenseVoice => 0,
            ModelKind::FunAsrNano => 1,
            ModelKind::Qwen3Asr => 2,
        });
        window.set_model_name(match startup_model {
            ModelKind::SenseVoice => "SenseVoice-Small".into(),
            ModelKind::FunAsrNano => "Fun-ASR-Nano".into(),
            ModelKind::Qwen3Asr => "Qwen3-ASR".into(),
        });
        window.set_model_capability_text(model_capability_text(startup_model).into());
        if let Ok(mut active) = active_model.lock() {
            *active = startup_model;
        }
    }
    command_sender.send(AsrCommand::LoadModel(ModelSelection {
        kind: startup_model,
    }))?;

    let sender_for_model = command_sender.clone();
    let state_for_model = Arc::clone(&state);
    window.on_switch_model(move |index| {
        let kind = match index {
            1 => crate::asr::ModelKind::FunAsrNano,
            2 => crate::asr::ModelKind::Qwen3Asr,
            _ => crate::asr::ModelKind::SenseVoice,
        };
        let should_load = state_for_model
            .lock()
            .map(|mut state| state.begin_model_loading())
            .unwrap_or(false);
        if should_load {
            let _ = sender_for_model.send(AsrCommand::LoadModel(ModelSelection { kind }));
        }
    });

    let binding = HotkeyBinding::parse(&settings.hotkey.keys, settings.hotkey.suppress)?;
    let (hook_event_sender, hook_event_receiver) = bounded(128);
    let keyboard_hook = Arc::new(KeyboardHook::install(hook_event_sender, binding.clone())?);
    let (hotkey_control_sender, hotkey_control_receiver) = bounded(4);
    let hook_for_capture = Arc::clone(&keyboard_hook);
    let hotkey_control_for_capture = hotkey_control_sender.clone();
    let weak_window_for_capture = window.as_weak();
    window.on_capture_hotkey(move || {
        tracing::info!("local hotkey capture requested");
        hook_for_capture.start_capture();
        let _ = hotkey_control_for_capture.send(HotkeyControl::StartCapture);
        if let Some(window) = weak_window_for_capture.upgrade() {
            window.set_capturing_hotkey(true);
            window.set_status_text("请按下快捷键组合，全部松开后自动完成；按 Esc 取消".into());
        }
    });
    let hotkey_control_for_cancel = hotkey_control_sender.clone();
    let hook_for_cancel = Arc::clone(&keyboard_hook);
    let weak_window_for_cancel = window.as_weak();
    window.on_cancel_hotkey_capture(move || {
        let _ = hotkey_control_for_cancel.send(HotkeyControl::CancelCapture);
        hook_for_cancel.stop_capture();
        if let Some(window) = weak_window_for_cancel.upgrade() {
            window.set_capturing_hotkey(false);
            window.set_status_text("已取消快捷键捕获".into());
        }
    });
    let state_for_hotkey = Arc::clone(&state);
    let sender_for_hotkey = command_sender.clone();
    let audio_for_hotkey = audio_command_sender.clone();
    let overlay_for_hotkey = overlay_sender.clone();
    let hotwords = Arc::new(Mutex::new(
        settings
            .hotwords
            .items
            .iter()
            .map(|text| crate::asr::Hotword {
                text: text.clone(),
                boost: settings.hotwords.boost,
            })
            .collect::<Vec<_>>(),
    ));
    let hotwords_for_hotkey = Arc::clone(&hotwords);
    let state_for_model_test = Arc::clone(&state);
    let session_for_model_test = Arc::clone(&model_test_session);
    let asr_for_model_test = command_sender.clone();
    let audio_for_model_test = audio_command_sender.clone();
    let hotwords_for_model_test = Arc::clone(&hotwords);
    let weak_window_for_model_test = window.as_weak();
    window.on_toggle_model_test(move || {
        tracing::info!("model test button clicked");
        let Some(window) = weak_window_for_model_test.upgrade() else {
            return;
        };
        let current = session_for_model_test
            .lock()
            .ok()
            .and_then(|session| *session);
        if let Some(session_id) = current {
            tracing::info!(session_id, "stopping model test recording");
            let should_stop = state_for_model_test
                .lock()
                .map(|mut state| state.begin_finalizing(session_id))
                .unwrap_or(false);
            if should_stop {
                let _ = audio_for_model_test.send(AudioCommand::Stop { session_id });
                window.set_model_test_state(2);
                window.set_model_test_result("录音结束，正在识别...".into());
            }
            return;
        }

        let session_id = state_for_model_test
            .lock()
            .ok()
            .and_then(|mut state| state.start_session());
        let Some(session_id) = session_id else {
            window.set_model_test_result("模型尚未就绪，请稍后再试".into());
            return;
        };
        tracing::info!(session_id, "starting model test recording");
        let session_hotwords = hotwords_for_model_test
            .lock()
            .map(|hotwords| hotwords.clone())
            .unwrap_or_default();
        if let Ok(mut session) = session_for_model_test.lock() {
            *session = Some(session_id);
        }
        let _ = asr_for_model_test.send(AsrCommand::StartSession {
            session_id,
            hotwords: session_hotwords,
            enable_partials: false,
        });
        let _ = audio_for_model_test.send(AudioCommand::Start {
            session_id,
            segment_on_silence: false,
        });
        tracing::info!(session_id, "model test start commands sent");
        window.set_model_test_state(1);
        window.set_model_test_result("正在录音，请说话后点击停止".into());
    });
    let hook_for_coordinator = Arc::clone(&keyboard_hook);
    let weak_window_for_coordinator = window.as_weak();
    std::thread::Builder::new()
        .name("hotkey-coordinator".into())
        .spawn(move || {
            let mut current_binding = binding;
            let mut hotkey = HotkeyStateMachine::new(current_binding.clone());
            let mut capture_mode = false;
            let mut capture_pressed: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
            let mut capture_candidate: Vec<String> = Vec::new();
            loop {
                crossbeam_channel::select! {
                    recv(hotkey_control_receiver) -> control => {
                        let Ok(control) = control else {
                            break;
                        };
                        match control {
                            HotkeyControl::UpdateBinding(new_binding) => {
                                current_binding = new_binding.clone();
                                hotkey = HotkeyStateMachine::new(new_binding);
                            }
                            HotkeyControl::StartCapture => {
                                capture_mode = true;
                                capture_pressed.clear();
                                capture_candidate.clear();
                                hotkey = HotkeyStateMachine::new(current_binding.clone());
                            }
                            HotkeyControl::CancelCapture => {
                                capture_mode = false;
                                capture_pressed.clear();
                                capture_candidate.clear();
                                hotkey = HotkeyStateMachine::new(current_binding.clone());
                            }
                        }
                    }
                    recv(hook_event_receiver) -> event => {
                        let Ok(event) = event else {
                            break;
                        };
                        if capture_mode {
                            const VK_ESCAPE: u32 = 0x1B;
                            if event.virtual_key == VK_ESCAPE && event.pressed {
                                capture_mode = false;
                                capture_pressed.clear();
                                capture_candidate.clear();
                                hook_for_coordinator.stop_capture();
                                let weak = weak_window_for_coordinator.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(window) = weak.upgrade() {
                                        window.set_capturing_hotkey(false);
                                        window.set_status_text("已取消快捷键捕获".into());
                                    }
                                });
                                continue;
                            }
                            if event.pressed {
                                if capture_pressed.insert(event.virtual_key) {
                                    if let Some(name) = vk_to_display_name(event.virtual_key)
                                        && !capture_candidate.contains(&name)
                                    {
                                        capture_candidate.push(name);
                                    }
                                }
                            } else {
                                capture_pressed.remove(&event.virtual_key);
                                if capture_pressed.is_empty() && !capture_candidate.is_empty() {
                                    capture_mode = false;
                                    hook_for_coordinator.stop_capture();
                                    let keys = std::mem::take(&mut capture_candidate);
                                    let weak = weak_window_for_coordinator.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(window) = weak.upgrade() {
                                            window.set_hotkey_text(keys.join(" + ").into());
                                            window.set_capturing_hotkey(false);
                                            window.set_status_text("快捷键已捕获，请保存设置".into());
                                        }
                                    });
                                }
                            }
                            continue;
                        }
                        match hotkey.handle(event) {
                            Some(HotkeyAction::StartListening) => {
                                let Ok(mut state) = state_for_hotkey.lock() else {
                                    continue;
                                };
                                let Some(session_id) = state.start_session() else {
                                    continue;
                                };
                                let session_hotwords = hotwords_for_hotkey
                                    .lock()
                                    .map(|hotwords| hotwords.clone())
                                    .unwrap_or_default();
                                let _ = sender_for_hotkey.send(AsrCommand::StartSession {
                                    session_id,
                                    hotwords: session_hotwords,
                                    enable_partials: false,
                                });
                                let _ = audio_for_hotkey.send(AudioCommand::Start {
                                    session_id,
                                    segment_on_silence: true,
                                });
                                let _ = overlay_for_hotkey.send(OverlayCommand::Listening {
                                    text: "正在聆听...".into(),
                                });
                            }
                            Some(HotkeyAction::StopListening) => {
                                let Ok(mut state) = state_for_hotkey.lock() else {
                                    continue;
                                };
                                let Some(session_id) = state.active_session_id else {
                                    continue;
                                };
                                if state.begin_finalizing(session_id) {
                                    let _ = overlay_for_hotkey.send(OverlayCommand::Finalizing {
                                        text: "正在整理文本...".into(),
                                    });
                                    let _ = audio_for_hotkey.send(AudioCommand::Stop { session_id });
                                }
                            }
                            None => {}
                        }
                    }
                }
            }
        })?;

    let settings_for_save = Arc::new(Mutex::new(settings.clone()));
    let store_for_save = settings_store.clone();
    let hotwords_for_save = Arc::clone(&hotwords);
    let hook_for_save = Arc::clone(&keyboard_hook);
    let output_for_save = output_command_sender.clone();
    let weak_window_for_save = window.as_weak();
    let selected_microphone_for_save = Arc::clone(&selected_microphone);
    let active_model_for_save = Arc::clone(&active_model);
    let state_for_save = Arc::clone(&state);
    window.on_save_settings(move |hotkey_text, hotwords_text, output_index, suppress, strip_punct| {
        let can_save = state_for_save
            .lock()
            .map(|state| {
                matches!(
                    state.app_state,
                    crate::state::AppState::Ready | crate::state::AppState::Error
                )
            })
            .unwrap_or(false);
        if !can_save {
            if let Some(window) = weak_window_for_save.upgrade() {
                window.set_status_text("录音或模型加载期间不能保存设置".into());
            }
            return;
        }
        let selected_model = active_model_for_save
            .lock()
            .map(|model| *model)
            .unwrap_or(crate::asr::ModelKind::SenseVoice);
        let keys = hotkey_text
            .split('+')
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let Ok(binding) = HotkeyBinding::parse(&keys, suppress) else {
            if let Some(window) = weak_window_for_save.upgrade() {
                window.set_status_text("快捷键格式无效".into());
            }
            return;
        };
        let hotword_items = hotwords_text
            .lines()
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let output_mode = match output_index {
            1 => OutputMode::ClipboardPaste,
            2 => OutputMode::CopyOnly,
            _ => OutputMode::Unicode,
        };
        let boost = settings_for_save
            .lock()
            .map(|settings| settings.hotwords.boost)
            .unwrap_or(1.0);
        let save_result = settings_for_save
            .lock()
            .map_err(|_| anyhow::anyhow!("settings lock poisoned"))
            .and_then(|mut settings| {
                settings.hotkey.keys = keys.clone();
                settings.hotkey.suppress = suppress;
                settings.hotwords.items = hotword_items.clone();
                settings.output.mode = output_mode.as_str().into();
                settings.output.strip_trailing_punctuation = strip_punct;
                settings.asr.model = selected_model;
                if let Ok(selected) = selected_microphone_for_save.lock()
                    && let Some((id, name)) = selected.as_ref()
                {
                    settings.audio.device_name = name.clone();
                    settings.audio.device_id = id.clone();
                }
                store_for_save.save(&settings)
            });
        if save_result.is_ok() {
            if let Ok(mut current_hotwords) = hotwords_for_save.lock() {
                *current_hotwords = hotword_items
                    .iter()
                    .map(|text| crate::asr::Hotword {
                        text: text.clone(),
                        boost,
                    })
                    .collect();
            }
            hook_for_save.update_binding(binding.clone());
            let _ = hotkey_control_sender.send(HotkeyControl::UpdateBinding(binding));
            let _ = output_for_save.send(OutputCommand::SetMode(output_mode));
            let _ = output_for_save.send(OutputCommand::SetStripPunctuation(strip_punct));
        }
        if let Some(window) = weak_window_for_save.upgrade() {
            window.set_status_text(match save_result {
                Ok(()) => "设置已保存，下次录音立即生效".into(),
                Err(error) => error.to_string().into(),
            });
        }
    });

    let sender_for_demo = command_sender.clone();
    let state_for_demo = Arc::clone(&state);
    window.on_run_demo(move || {
        let Ok(mut state) = state_for_demo.lock() else {
            return;
        };
        let Some(session_id) = state.start_session() else {
            return;
        };
        let _ = sender_for_demo.send(AsrCommand::StartSession {
            session_id,
            hotwords: Vec::new(),
            enable_partials: false,
        });
        if state.begin_finalizing(session_id) {
            let _ = sender_for_demo.send(AsrCommand::FinishSession { session_id });
        }
    });

    window.run()?;
    if let Ok(mut state) = state.lock() {
        state.app_state = crate::state::AppState::ShuttingDown;
        tracing::info!(state = state.app_state.label(), "application shutting down");
    }
    let _ = command_sender.send(AsrCommand::Shutdown);
    let _ = audio_command_sender.send(AudioCommand::Shutdown);
    let _ = output_command_sender.send(OutputCommand::Shutdown);
    worker.join();
    audio_worker.join();
    output_worker.join();
    overlay.shutdown();
    Ok(())
}

struct AsrEventContext<'a> {
    window: &'a slint::Weak<AppWindow>,
    state: &'a Arc<Mutex<SessionState>>,
    active_model: &'a Arc<Mutex<crate::asr::ModelKind>>,
    model_test_session: &'a Arc<Mutex<Option<u64>>>,
    output: &'a crossbeam_channel::Sender<OutputCommand>,
    overlay: &'a crossbeam_channel::Sender<OverlayCommand>,
    audio: &'a crossbeam_channel::Sender<AudioCommand>,
}

fn handle_asr_event(context: AsrEventContext<'_>, event: AsrEvent) {
    let AsrEventContext {
        window,
        state,
        active_model,
        model_test_session,
        output,
        overlay,
        audio,
    } = context;
    let Some(window) = window.upgrade() else {
        return;
    };

    match event {
        AsrEvent::ModelLoading(kind) => {
            tracing::info!(?kind, "loading ASR model");
            window.set_status_text("正在加载模型".into());
        }
        AsrEvent::ModelReady(model) => {
            tracing::info!(kind = ?model.kind, model = %model.display_name, "ASR model ready");
            if let Ok(mut state) = state.lock() {
                state.mark_ready();
            }
            if let Ok(mut active) = active_model.lock() {
                *active = model.kind;
            }
            window.set_model_index(match model.kind {
                crate::asr::ModelKind::SenseVoice => 0,
                crate::asr::ModelKind::FunAsrNano => 1,
                crate::asr::ModelKind::Qwen3Asr => 2,
            });
            window.set_model_name(model.display_name.into());
            window.set_model_capability_text(model_capability_text(model.kind).into());
            window.set_status_text("就绪".into());
            // Clear any stale error shown on the overlay from a previous failed load.
            let _ = overlay.send(OverlayCommand::Hide);
        }
        AsrEvent::ModelLoadFailed {
            kind,
            error,
            previous_model_available,
        } => {
            tracing::error!(?kind, %error, previous_model_available, "ASR model load failed");
            if let Ok(mut state) = state.lock() {
                if previous_model_available {
                    state.mark_ready();
                } else {
                    state.mark_error();
                }
            }
            let message = error.to_string();
            let active = active_model
                .lock()
                .map(|model| *model)
                .unwrap_or(crate::asr::ModelKind::SenseVoice);
            window.set_model_index(match active {
                crate::asr::ModelKind::SenseVoice => 0,
                crate::asr::ModelKind::FunAsrNano => 1,
                crate::asr::ModelKind::Qwen3Asr => 2,
            });
            let _ = overlay.send(OverlayCommand::Error {
                message: message.clone(),
            });
            window.set_status_text(message.into());
        }
        AsrEvent::Partial {
            session_id,
            text,
            revision,
        } => {
            let accepts = state
                .lock()
                .map(|state| {
                    state.accepts(session_id)
                        && state.app_state == crate::state::AppState::Listening
                })
                .unwrap_or(false);
            if accepts {
                tracing::debug!(
                    session_id,
                    revision,
                    text_length = text.chars().count(),
                    "ASR partial"
                );
                let _ = overlay.send(OverlayCommand::Listening { text: text.clone() });
                window.set_status_text(text.into());
            }
        }
        AsrEvent::Final {
            session_id,
            text,
            metrics,
        } => {
            tracing::info!(
                session_id,
                audio_duration_ms = metrics.audio_duration_ms,
                inference_duration_ms = metrics.inference_duration_ms,
                text_length = text.chars().count(),
                "ASR final"
            );
            let is_model_test = model_test_session
                .lock()
                .map(|session| *session == Some(session_id))
                .unwrap_or(false);
            if is_model_test {
                tracing::info!(session_id, "handling model test final");
                if let Ok(mut test_session) = model_test_session.lock() {
                    *test_session = None;
                }
                tracing::info!(session_id, "cleared model test session");
                if let Ok(mut state) = state.lock() {
                    state.complete_session(session_id);
                }
                tracing::info!(session_id, "completed model test state");
                window.set_model_test_state(0);
                window.set_model_test_result(
                    format!(
                        "识别结果：{}\n音频时长：{} ms\n推理耗时：{} ms",
                        if text.is_empty() {
                            "（未识别到文本）"
                        } else {
                            &text
                        },
                        metrics.audio_duration_ms,
                        metrics.inference_duration_ms,
                    )
                    .into(),
                );
                window.set_status_text("模型测试完成".into());
                tracing::info!(session_id, "updated model test UI");
                return;
            }
            if text.is_empty() {
                if let Ok(mut state) = state.lock() {
                    state.complete_session(session_id);
                }
                let _ = overlay.send(OverlayCommand::Hide);
                window.set_status_text("就绪".into());
                return;
            }
            let should_output = state
                .lock()
                .map(|mut state| state.begin_injecting(session_id))
                .unwrap_or(false);
            if should_output {
                window.set_status_text("正在上屏".into());
                let _ = overlay.send(OverlayCommand::Injecting { text: text.clone() });
                let _ = output.send(OutputCommand::Write { session_id, text });
            }
        }
        AsrEvent::SessionCancelled { session_id } => {
            tracing::info!(session_id, "ASR session cancellation completed");
            if let Ok(mut state) = state.lock()
                && state.app_state == crate::state::AppState::Error
            {
                state.mark_ready();
            }
        }
        AsrEvent::Error { session_id, error } => {
            if let Some(session_id) = session_id {
                let _ = audio.send(AudioCommand::Stop { session_id });
                if let Ok(mut state) = state.lock() {
                    state.complete_session(session_id);
                }
                if let Ok(mut test_session) = model_test_session.lock()
                    && *test_session == Some(session_id)
                {
                    *test_session = None;
                    window.set_model_test_state(0);
                    window.set_model_test_result(error.to_string().into());
                }
            }
            let _ = overlay.send(OverlayCommand::Error {
                message: error.to_string(),
            });
            window.set_status_text(error.to_string().into());
        }
    }
}

fn model_capability_text(kind: crate::asr::ModelKind) -> &'static str {
    match kind {
        crate::asr::ModelKind::SenseVoice => "ONNX CPU 推理 · CTC Top-K 热词增强",
        crate::asr::ModelKind::FunAsrNano => "ONNX + llama.cpp · 热词进入最终 LLM 提示词",
        crate::asr::ModelKind::Qwen3Asr => "ONNX + llama.cpp · 实验性转写上下文词",
    }
}

fn handle_output_event(
    window: &slint::Weak<AppWindow>,
    state: &Arc<Mutex<SessionState>>,
    overlay: &crossbeam_channel::Sender<OverlayCommand>,
    event: OutputEvent,
) {
    let Some(window) = window.upgrade() else {
        return;
    };

    match event {
        OutputEvent::Written { session_id } => {
            if let Ok(mut state) = state.lock() {
                state.complete_session(session_id);
            }
            let _ = overlay.send(OverlayCommand::Hide);
            window.set_status_text("就绪".into());
        }
        OutputEvent::Retained {
            session_id,
            text,
            reason,
        } => {
            if let Ok(mut state) = state.lock() {
                state.complete_session(session_id);
            }
            let _ = overlay.send(OverlayCommand::Hide);
            window.set_status_text(format!("{reason}，结果已保留：{text}").into());
        }
    }
}
