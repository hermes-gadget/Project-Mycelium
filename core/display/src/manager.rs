use std::collections::HashMap;

use anyhow::{bail, Result};
use mycelium_input::{register_input_manager, remove_input_manager, SharedInputManager};
use sdl2::event::Event;
use sdl2::{EventPump, Sdl};

use crate::window::{event_for_window, DisplayEvent, DisplayWindow, Rect};
use crate::DisplayConfig;

pub struct DisplayManager {
    windows: HashMap<String, DisplayWindow>,
    input_managers: HashMap<String, SharedInputManager>,
    sdl_context: Sdl,
    event_pump: EventPump,
}

impl DisplayManager {
    pub fn new() -> Result<Self> {
        let sdl_context = sdl2::init().map_err(anyhow::Error::msg)?;
        let event_pump = sdl_context.event_pump().map_err(anyhow::Error::msg)?;
        Ok(Self {
            windows: HashMap::new(),
            input_managers: HashMap::new(),
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
        self.input_managers.insert(
            instance_id.to_owned(),
            register_input_manager(instance_id, config.scale as f32),
        );
        Ok(())
    }

    pub fn destroy_window(&mut self, instance_id: &str) {
        self.windows.remove(instance_id);
        self.input_managers.remove(instance_id);
        remove_input_manager(instance_id);
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
            display_events.extend(self.handle_event(event));
        }
        display_events
    }

    fn handle_event(&mut self, event: Event) -> Vec<DisplayEvent> {
        if matches!(event, Event::Quit { .. }) {
            return self
                .windows
                .keys()
                .cloned()
                .map(|instance_id| DisplayEvent::Close { instance_id })
                .collect();
        }
        let Some(window_id) = event.get_window_id() else {
            return Vec::new();
        };
        let Some(instance_id) = self
            .windows
            .values()
            .find(|window| window.window_id() == window_id)
            .map(|window| window.id.clone())
        else {
            return Vec::new();
        };

        if let Some(manager) = self.input_managers.get(&instance_id) {
            manager
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .handle_sdl_event(&event);
        }

        event_for_window(&instance_id, window_id, event)
            .into_iter()
            .collect()
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

    #[test]
    fn routes_window_input_to_the_matching_instance() {
        let _serial = crate::SDL_TEST_LOCK.lock().unwrap();
        std::env::set_var("SDL_VIDEODRIVER", "dummy");
        std::env::set_var("SDL_RENDER_DRIVER", "software");
        let mut manager = DisplayManager::new().unwrap();
        let config = DisplayConfig {
            scale: 1,
            ..DisplayConfig::default()
        };
        manager.create_window("node1", config.clone()).unwrap();
        manager.create_window("node2", config).unwrap();
        let node1_window = manager.windows["node1"].window_id();

        manager.handle_event(Event::KeyDown {
            timestamp: 0,
            window_id: node1_window,
            keycode: Some(sdl2::keyboard::Keycode::Q),
            scancode: None,
            keymod: sdl2::keyboard::Mod::NOMOD,
            repeat: false,
        });

        let node1 = manager.input_managers["node1"].lock().unwrap();
        let mut node2 = manager.input_managers["node2"].lock().unwrap();
        assert_eq!(node1.keyboard.get_last().unwrap().row, 0);
        assert!(node2.poll_keyboard().is_none());
    }
}
