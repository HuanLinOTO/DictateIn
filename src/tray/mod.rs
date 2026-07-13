use anyhow::Result;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    ShowSettings,
    Exit,
}

pub struct AppTray {
    _tray_icon: TrayIcon,
}

impl AppTray {
    pub fn create(command_sender: crossbeam_channel::Sender<TrayCommand>) -> Result<Self> {
        let menu = Menu::new();
        let show_item = MenuItem::new("打开设置", true, None);
        let exit_item = MenuItem::new("退出", true, None);
        menu.append(&show_item)?;
        menu.append(&exit_item)?;

        let icon = create_icon()?;
        let tray_icon = TrayIconBuilder::new()
            .with_tooltip("DictateIn")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .build()?;

        let show_id = show_item.id().clone();
        let exit_id = exit_item.id().clone();
        std::thread::Builder::new()
            .name("tray-events".into())
            .spawn(move || {
                loop {
                    crossbeam_channel::select! {
                        recv(MenuEvent::receiver()) -> event => {
                            let Ok(event) = event else {
                                break;
                            };
                            let command = if event.id == show_id {
                                Some(TrayCommand::ShowSettings)
                            } else if event.id == exit_id {
                                Some(TrayCommand::Exit)
                            } else {
                                None
                            };
                            if let Some(command) = command {
                                let _ = command_sender.send(command);
                            }
                        }
                        recv(TrayIconEvent::receiver()) -> event => {
                            let Ok(event) = event else {
                                break;
                            };
                            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                                let _ = command_sender.send(TrayCommand::ShowSettings);
                            }
                        }
                    }
                }
            })?;

        Ok(Self {
            _tray_icon: tray_icon,
        })
    }
}

fn create_icon() -> Result<Icon> {
    const SIZE: u32 = 32;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);

    for y in 0..SIZE {
        for x in 0..SIZE {
            let center_x = x as i32 - 16;
            let center_y = y as i32 - 16;
            let inside = center_x * center_x + center_y * center_y <= 14 * 14;
            let waveform = (x as i32 - 16).abs() <= 2
                || ((x as i32 - 9).abs() <= 1 && (y as i32 - 16).abs() <= 6)
                || ((x as i32 - 23).abs() <= 1 && (y as i32 - 16).abs() <= 6);

            if inside && waveform {
                rgba.extend_from_slice(&[229, 107, 63, 255]);
            } else if inside {
                rgba.extend_from_slice(&[22, 125, 146, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    Ok(Icon::from_rgba(rgba, SIZE, SIZE)?)
}
