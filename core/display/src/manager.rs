use std::collections::HashMap;

use anyhow::{bail, Result};
use sdl2::{EventPump, Sdl};

use crate::window::{event_for_window, DisplayEvent, DisplayWindow, Rect};
use crate::DisplayConfig;

pub struct DisplayManager {
    windows: HashMap<String, DisplayWindow>,
    sdl_context: Sdl,
    event_pump: EventPump,
}

impl DisplayManager {
    pub fn new() -> Result<Self> {
        let sdl_context = sdl2::init().map_err(anyhow::Error::msg)?;
        let event_pump = sdl_context.event_pump().map_err(anyhow::Error::msg)?;
        Ok(Self {
            windows: HashMap::new(),
            sdl_context,
            event_pump,
        })
    }

    pub fn create_window(&mut self, instance_id: &str, config: DisplayConfig) -> Result<()> {
        if instance_id.is_empty() {
            bail!("instance ID cannot be empty");
        }
        if self.windows.contains_key(instance_id) {
            bail!("display window {instance_id} already exists");
        }

        let video = self.sdl_context.video().map_err(anyhow::Error::msg)?;
        let title = format!("T-Deck — {instance_id}");
        let mut window = DisplayWindow::create_with_video(
            &video,
            &title,
            config.width,
            config.height,
            config.scale,
        )?;
        window.id = instance_id.to_owned();
        self.windows.insert(instance_id.to_owned(), window);
        Ok(())
    }

    pub fn destroy_window(&mut self, instance_id: &str) {
        self.windows.remove(instance_id);
    }

    pub fn present_framebuffer(
        &mut self,
        instance_id: &str,
        data: &[u8],
        area: Rect,
    ) -> Result<()> {
        let Some(window) = self.windows.get_mut(instance_id) else {
            bail!("display window {instance_id} does not exist");
        };
        window.present_rgb565(data, area.x, area.y, area.width, area.height)
    }

    pub fn capture_screenshot(&mut self, instance_id: &str) -> Result<Vec<u8>> {
        let Some(window) = self.windows.get(instance_id) else {
            bail!("display window {instance_id} does not exist");
        };
        let screenshot = window.capture_screenshot();
        if screenshot.is_empty() {
            bail!("failed to encode screenshot for {instance_id}");
        }
        Ok(screenshot)
    }

    pub fn capture_rgb565(&self, instance_id: &str) -> Result<Vec<u8>> {
        let Some(window) = self.windows.get(instance_id) else {
            bail!("display window {instance_id} does not exist");
        };
        Ok(window.capture_rgb565())
    }

    pub fn list_windows(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.windows.keys().cloned().collect();
        ids.sort_unstable();
        ids
    }

    pub fn handle_events(&mut self) -> Vec<DisplayEvent> {
        let events: Vec<_> = self.event_pump.poll_iter().collect();
        let mut display_events = Vec::new();
        for event in events {
            if matches!(event, sdl2::event::Event::Quit { .. }) {
                display_events.extend(
                    self.windows
                        .keys()
                        .cloned()
                        .map(|instance_id| DisplayEvent::Close { instance_id }),
                );
                continue;
            }
            let Some(window_id) = event.get_window_id() else {
                continue;
            };
            if let Some(window) = self
                .windows
                .values()
                .find(|window| window.window_id() == window_id)
            {
                if let Some(event) = event_for_window(&window.id, window_id, event) {
                    display_events.push(event);
                }
            }
        }
        display_events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manages_multiple_window_lifecycles() {
        let _serial = crate::SDL_TEST_LOCK.lock().unwrap();
        std::env::set_var("SDL_VIDEODRIVER", "dummy");
        std::env::set_var("SDL_RENDER_DRIVER", "software");
        let mut manager = DisplayManager::new().unwrap();
        let config = DisplayConfig {
            width: 4,
            height: 3,
            scale: 1,
            ..DisplayConfig::default()
        };

        manager.create_window("node2", config.clone()).unwrap();
        manager.create_window("node1", config.clone()).unwrap();
        assert_eq!(manager.list_windows(), ["node1", "node2"]);
        assert!(manager.create_window("node1", config).is_err());

        let frame = vec![0xff; 4 * 3 * 2];
        manager
            .present_framebuffer(
                "node1",
                &frame,
                Rect {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 3,
                },
            )
            .unwrap();
        assert_eq!(manager.capture_rgb565("node1").unwrap(), frame);
        assert!(manager
            .capture_screenshot("node1")
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n"));

        manager.destroy_window("node1");
        assert_eq!(manager.list_windows(), ["node2"]);
        assert!(manager.capture_screenshot("node1").is_err());
    }
}
