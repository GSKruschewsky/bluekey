use arc_swap::ArcSwap;
use evdev::{Device, EventStream, EventSummary, InputEvent, KeyCode, RelativeAxisCode};
use std::{borrow::Borrow, sync::Arc, time::{Duration, Instant}};
use tokio::{sync::oneshot, task::JoinHandle};

use crate::bluetooth::{ReturnError, Target, keyboard::{Keyboard, KeyboardReturnEvent, KeyboardServerDied}, leds::Led, mouse::{Button, Mouse, MouseServerDied}};


#[derive(Debug)]
pub enum EvdevBridgeError {
    #[allow(unused)]
    EvdevError(std::io::Error),
    ServerDied,
    Desynced
}
impl From<std::io::Error> for EvdevBridgeError {
    fn from(value: std::io::Error) -> Self {
        EvdevBridgeError::EvdevError(value)
    }
}
impl From<KeyboardServerDied> for EvdevBridgeError {
    fn from(_value: KeyboardServerDied) -> Self {
        EvdevBridgeError::ServerDied
    }
}
impl From<ReturnError> for EvdevBridgeError {
    fn from(value: ReturnError) -> Self {
        match value {
            ReturnError::Lagged(_) => EvdevBridgeError::Desynced,
            ReturnError::ServerDied => EvdevBridgeError::ServerDied
        }
    }
}


impl From<MouseServerDied> for EvdevBridgeError {
    fn from(_value: MouseServerDied) -> Self {
        EvdevBridgeError::ServerDied
    }
}

#[derive(Clone)]
pub struct SharedShortcut {
    shortcut: Arc<ArcSwap<Vec<KeyCode>>>
}
impl SharedShortcut {
    pub fn new(shortcut: Arc<Vec<KeyCode>>) -> Self {
        Self {
            shortcut: Arc::new(ArcSwap::new(shortcut))
        }
    }

    pub fn update(&self, shortcut: Arc<Vec<KeyCode>>) {
        self.shortcut.swap(shortcut);
    }

    pub fn shortcut(&self) -> Shortcut {
        Shortcut::new(self)
    }

    pub fn keys(&self) -> Arc<Vec<KeyCode>> {
        self.shortcut.load().clone()
    }
}

#[derive(Debug, Clone)]
pub struct Shortcut {
    shortcut: Arc<ArcSwap<Vec<KeyCode>>>, // This is cursed
    active: Arc<Vec<KeyCode>>,
    progress: usize
}
impl Shortcut {
    pub fn new(shared: &SharedShortcut) -> Self {
        Self {
            shortcut: shared.shortcut.clone(),
            active: shared.shortcut.load().clone(),
            progress: 0
        }
    }

    fn pressed(&mut self, keycode: KeyCode) -> Match {
        if self.active[self.progress] == keycode {
            self.progress += 1;

            if self.progress == self.active.len() {
                self.progress = 0;
                return Match::Complete
            }
            return Match::Partial
        }
        Match::No
    }
    fn released(&mut self, keycode: KeyCode) {
        if self.active[..=self.progress].contains(&keycode) {
            self.progress = 0;
        }

        if self.progress == 0 {
            self.active = self.shortcut.load().clone();
        }
    }

    pub fn keys(&self) -> &[KeyCode] {
        &self.active
    }
}

/// What a key press does to the escape shortcut
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Match {
    /// It's the next key of the shortcut, but more are needed
    Partial,
    /// It's the shortcut's last key
    Complete,
    /// It isn't the next key of the shortcut
    No
}

/// Keeps the escape shortcut from reaching the target. A press that matches the shortcut so far is held back until a
/// later key shows whether the shortcut is being typed: if it's completed, the target never sees any of it, otherwise
/// the held presses are sent first. Forwarding them straight away made Super+Esc reach the target as a lone Super tap,
/// which Android takes as "open the app drawer", leaving whatever app was on screen.
#[derive(Debug)]
struct EscapeFilter {
    shortcut: Shortcut,
    /// Presses matching the shortcut so far, not sent yet
    held: Vec<KeyCode>
}

#[derive(Debug, PartialEq, Eq)]
enum Filtered {
    /// Send these on, in order(true for a press, false for a release)
    Send(Vec<(KeyCode, bool)>),
    /// The shortcut was completed. These keys of it reached the target and need releasing there
    Escaped(Vec<KeyCode>)
}

impl EscapeFilter {
    fn new(shortcut: Shortcut) -> Self {
        Self {
            shortcut,
            held: Vec::new()
        }
    }

    /// Takes an evdev key event's value: 1 for a press, 0 for a release, 2 for a repeat(ignored)
    fn key(&mut self, code: KeyCode, action: i32) -> Filtered {
        match action {
            1 => match self.shortcut.pressed(code) {
                Match::Complete => {
                    let held = std::mem::take(&mut self.held);
                    Filtered::Escaped(self.shortcut.keys().iter()
                        .filter(|key| **key != code && !held.contains(key))
                        .copied()
                        .collect())
                },
                Match::Partial => {
                    self.held.push(code);
                    Filtered::Send(Vec::new())
                },
                Match::No => {
                    let mut send = self.take_held();
                    send.push((code, true));
                    Filtered::Send(send)
                }
            },
            0 => {
                self.shortcut.released(code);
                // Once the shortcut is abandoned, what was held back is just keys being used
                let mut send = match self.shortcut.progress {
                    0 => self.take_held(),
                    _ => Vec::new()
                };
                send.push((code, false));
                Filtered::Send(send)
            },
            _ => Filtered::Send(Vec::new())
        }
    }

    /// The held presses, to send now, in the order they happened
    fn take_held(&mut self) -> Vec<(KeyCode, bool)> {
        self.held.drain(..).map(|key| (key, true)).collect()
    }
}



struct Cancel;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BreakReason {
    Cancelled,
    Escaped
}


pub struct KeyboardBridge {
    canceller: Option<oneshot::Sender<Cancel>>,
    handle: JoinHandle<Result<BreakReason, EvdevBridgeError>>
}
impl KeyboardBridge {
    pub fn start<T: Borrow<Keyboard> + Send + 'static, F: FnOnce() -> R + Send + 'static, R: Future<Output=()> + Send>(keyboard: T, device: EventStream, target: Target, escape: Shortcut, notify: F) -> Self {
        let (canceller, cancelee) = oneshot::channel();
        let handle = tokio::spawn(async move {
            let result = evdev_keyboard_bridge(keyboard, device, target, cancelee, escape).await;
            if !matches!(result, Ok(BreakReason::Cancelled)) {
                notify().await;
            }
            result
        });        
        
        Self {
            handle,
            canceller: Some(canceller)
        }
    }
    pub async fn cancel(mut self) -> Result<BreakReason, EvdevBridgeError> {
        if let Some(canceller) = self.canceller.take() {
            let _ = canceller.send(Cancel); // If the channel is closed, the bridge has stoped and handle has a value
        }

        (&mut self.handle).await.unwrap() // Propgate panics
    }
    pub async fn wait_for_break(mut self) -> Result<BreakReason, EvdevBridgeError> {
        (&mut self.handle).await.unwrap() // Propgate panics
    }
}
impl Drop for KeyboardBridge {
    fn drop(&mut self) {
        if let Some(canceller) = self.canceller.take() {
            let _ = canceller.send(Cancel); // If the channel is closed, the bridge has stoped and handle has a value
        }
    }
}


async fn evdev_keyboard_bridge<T: Borrow<Keyboard> + Send + 'static>(keyboard: T, mut device: EventStream, target: Target, mut cancel: oneshot::Receiver<Cancel>, escape: Shortcut) -> Result<BreakReason, EvdevBridgeError> {
    let keyboard = keyboard.borrow();
    let mut escape = EscapeFilter::new(escape);

    let mut returns = match target {
        Target::Target(target) => Some(target),
        Target::Broadcast => None
    };

    let mut keyboard_events = keyboard.listen();
    // Load keyboard's current LED events
    if let Target::Target(address) = target {
        keyboard.devices()?.get(address, |state| {
            if let Some(state) = state {
                for (led, state) in state.led_state() {
                    set_led(device.device_mut(), led, state)?;
                }
            }
            Ok::<(), EvdevBridgeError>(())
        }).await?
    };

    loop {
        tokio::select! {
            event = device.next_event() => {
                if let EventSummary::Key(_, code, action) = event?.destructure() {
                    match escape.key(code, action) {
                        Filtered::Escaped(sent) => {
                            for code in sent.into_iter().filter_map(usb_code) {
                                keyboard.release(target, code).await?
                            }
                            break Ok(BreakReason::Escaped)
                        },
                        Filtered::Send(keys) => for (code, pressed) in keys {
                            let Some(code) = usb_code(code) else { continue };
                            match pressed {
                                true => keyboard.press(target, code).await?,
                                false => keyboard.release(target, code).await?
                            }
                        }
                    }
                }
            },
            event = keyboard_events.next_event() => if let Some(target) = returns {
                match event? {
                    KeyboardReturnEvent::LedOn(from, led)  if from==target => set_led(device.device_mut(), led, true)?,
                    KeyboardReturnEvent::LedOff(from, led) if from==target => set_led(device.device_mut(), led, false)?,
                    KeyboardReturnEvent::Register(address) if returns == None => returns = Some(address),
                    _ => ()
                }                
            },
            _ = &mut cancel => return Ok(BreakReason::Cancelled)
        }
    }
}
/// The USB HID usage for an evdev key, if it has one
fn usb_code(key: KeyCode) -> Option<u8> {
    let map = keycode::KeyMap::from_key_mapping(keycode::KeyMapping::Evdev(key.0)).ok()?;
    Some(map.usb.try_into().expect("USB scancode is always 8 bits"))
}
fn set_led(device: &mut Device, led: Led, on: bool) -> Result<(), std::io::Error> {
    let on = match on {
        true => 1,
        false => 0
    };

    if let Some(id) = led.ev_dev_id() {
        device.send_events(&[
            InputEvent::new(evdev::EventType::LED.0, id.into(), on)
        ])?;
    };

    Ok(())
    
}

struct Clock {
    duration: Duration,
    last: Instant
}
impl Clock {
    fn new(duration: Duration) -> Self {
        Clock {
            duration,
            last: Instant::now()
        }
    }
    async fn next(&mut self, instant: Instant) {
        let remaining = self.duration.saturating_sub(instant.saturating_duration_since(self.last));
        
        if remaining > Duration::new(0, 0) {
            tokio::time::sleep(remaining).await;
        }
        self.last = Instant::now();
    }
}

fn map_button_codes(button: KeyCode) -> Option<Button> {
    match button {
        KeyCode::BTN_LEFT => Some(Button::from_id(1).unwrap()),
        KeyCode::BTN_RIGHT => Some(Button::from_id(2).unwrap()),
        KeyCode::BTN_MIDDLE => Some(Button::from_id(3).unwrap()),
        _ => None
    }
}

pub struct MouseBridge {
    canceller: Option<oneshot::Sender<Cancel>>,
    handle: JoinHandle<Result<(), EvdevBridgeError>>
}
impl MouseBridge {
    pub fn start<T: Borrow<Mouse> + Send + 'static>(mouse: T, device: EventStream, target: Target) -> Self {
        let (canceller, cancelee) = oneshot::channel();
        let handle = tokio::spawn(evdev_mouse_bridge(mouse, device, target, cancelee));
        Self {
            handle,
            canceller: Some(canceller)
        }
    }
    pub async fn cancel(mut self) -> Result<(), EvdevBridgeError> {
        if let Some(canceller) = self.canceller.take() {
            let _ = canceller.send(Cancel); // If the channel is closed, the bridge has stoped and handle has a value
        }
        (&mut self.handle).await.unwrap() // Propgate panics
    }
    pub async fn wait_for_break(mut self) -> Result<(), EvdevBridgeError> {
        (&mut self.handle).await.unwrap()
    }
}
impl Drop for MouseBridge {
    fn drop(&mut self) {
        if let Some(canceller) = self.canceller.take() {
            let _ = canceller.send(Cancel); // If the channel is closed, the bridge has stoped and handle has a value
        }
    }
}


async fn evdev_mouse_bridge<T: Borrow<Mouse> + Send + 'static>(mouse: T, mut device: EventStream, target: Target, mut canceller: oneshot::Receiver<Cancel>) -> Result<(), EvdevBridgeError> {  
    let mouse = mouse.borrow();

    // Immeidately sending all mouse events caused horrific lag and queue backup
    // The optimal time here depends on the connection interval, which, as far as I know, BlueZ provides no easy way to find
    // Any faster than the connection interval, and reports start backing up. This *could* be improved using a Bluetooth Classic device,
    // but that is a *major* rewrite. 
    let mut movement_clock = Clock::new(Duration::from_millis(30));
    let mut log_clock = Clock::new(Duration::from_millis(1000));
    let mut x = 0;
    let mut y = 0;
    let mut scroll = 0;

    #[derive(Debug)]
    enum Event {
        Evdev(EventSummary),
        MouseClock,
        LogClock,
        Cancel
    }
    let mut next_event = async || {
        tokio::select! {
            event = device.next_event() => match event {
                Ok(event) => Ok(Event::Evdev(event.destructure())),
                Err(error) => Err(EvdevBridgeError::EvdevError(error))
            },
            _ = movement_clock.next(std::time::Instant::now()) => Ok(Event::MouseClock),
            _ = log_clock.next(std::time::Instant::now()) => Ok(Event::LogClock),
            _ = &mut canceller => Ok(Event::Cancel)
        }
    };

    loop {
        let event = next_event().await?;
        match event {
            Event::Evdev(EventSummary::Key(_, code, action)) if let Some(button) = map_button_codes(code) => match action {
                0 => mouse.release(target, button).await?,
                1 => mouse.press(target, button).await?,
                _ => ()
            },
            Event::Evdev(EventSummary::RelativeAxis(_, RelativeAxisCode::REL_X, amount)) => {
                x += amount;
            },
            Event::Evdev(EventSummary::RelativeAxis(_, RelativeAxisCode::REL_Y, amount)) => {
                y += amount;
            },
            Event::Evdev(EventSummary::RelativeAxis(_, RelativeAxisCode::REL_WHEEL, amount)) => {
                scroll += amount;
            },
            Event::Evdev(_) => (),

            Event::MouseClock => {
                let mx = x.clamp(i8::MIN as i32, i8::MAX as i32) as i8;
                let my = y.clamp(i8::MIN as i32, i8::MAX as i32) as i8;
                let ms = scroll.clamp(i8::MIN as i32, i8::MAX as i32) as i8;

                mouse.moved(target, mx, my, ms).await?;
                x -= mx as i32;
                y -= my as i32;
                scroll = 0;
            },
            Event::LogClock => {
                //println!("Avg delta: {}ms, behind: {}ms", delta_sum.as_millis()/(events as u128).max(1), std::time::SystemTime::now().duration_since(latest).unwrap_or(Duration::new(0,0)).as_millis())
            },
            Event::Cancel => return Ok(())

            
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPER: KeyCode = KeyCode::KEY_LEFTMETA;
    const ESC: KeyCode = KeyCode::KEY_ESC;
    const TAB: KeyCode = KeyCode::KEY_TAB;
    const A: KeyCode = KeyCode::KEY_A;

    fn filter(keys: &[KeyCode]) -> EscapeFilter {
        EscapeFilter::new(SharedShortcut::new(Arc::new(keys.to_vec())).shortcut())
    }
    fn press(filter: &mut EscapeFilter, key: KeyCode) -> Filtered {
        filter.key(key, 1)
    }
    fn release(filter: &mut EscapeFilter, key: KeyCode) -> Filtered {
        filter.key(key, 0)
    }
    fn send(keys: &[(KeyCode, bool)]) -> Filtered {
        Filtered::Send(keys.to_vec())
    }

    #[test]
    fn the_target_never_sees_the_shortcut() {
        let mut escape = filter(&[SUPER, ESC]);
        assert_eq!(press(&mut escape, SUPER), send(&[]));
        assert_eq!(escape.key(SUPER, 2), send(&[])); // Holding Super repeats it
        assert_eq!(press(&mut escape, ESC), Filtered::Escaped(vec![]));
    }

    #[test]
    fn a_lone_super_tap_still_reaches_the_target() {
        let mut escape = filter(&[SUPER, ESC]);
        assert_eq!(press(&mut escape, SUPER), send(&[]));
        assert_eq!(release(&mut escape, SUPER), send(&[(SUPER, true), (SUPER, false)]));
    }

    #[test]
    fn other_super_shortcuts_reach_the_target_in_order() {
        let mut escape = filter(&[SUPER, ESC]);
        assert_eq!(press(&mut escape, SUPER), send(&[]));
        assert_eq!(press(&mut escape, TAB), send(&[(SUPER, true), (TAB, true)]));
        assert_eq!(release(&mut escape, TAB), send(&[(TAB, false)]));
        assert_eq!(release(&mut escape, SUPER), send(&[(SUPER, false)]));
    }

    #[test]
    fn escaping_after_another_super_shortcut_releases_super_on_the_target() {
        let mut escape = filter(&[SUPER, ESC]);
        press(&mut escape, SUPER);
        assert_eq!(press(&mut escape, TAB), send(&[(SUPER, true), (TAB, true)]));
        assert_eq!(release(&mut escape, TAB), send(&[(TAB, false)]));
        assert_eq!(press(&mut escape, ESC), Filtered::Escaped(vec![SUPER]));
    }

    #[test]
    fn keys_outside_the_shortcut_pass_straight_through() {
        let mut escape = filter(&[SUPER, ESC]);
        assert_eq!(press(&mut escape, A), send(&[(A, true)]));
        assert_eq!(escape.key(A, 2), send(&[]));
        assert_eq!(release(&mut escape, A), send(&[(A, false)]));
        // Esc on its own isn't the shortcut
        assert_eq!(press(&mut escape, ESC), send(&[(ESC, true)]));
        assert_eq!(release(&mut escape, ESC), send(&[(ESC, false)]));
    }

    #[test]
    fn abandoning_the_shortcut_sends_what_was_held() {
        let mut escape = filter(&[SUPER, ESC]);
        // Esc held down on the target, then Super pressed and Esc let go: Esc can't complete the shortcut anymore
        assert_eq!(press(&mut escape, ESC), send(&[(ESC, true)]));
        assert_eq!(press(&mut escape, SUPER), send(&[]));
        assert_eq!(release(&mut escape, ESC), send(&[(SUPER, true), (ESC, false)]));
        assert_eq!(release(&mut escape, SUPER), send(&[(SUPER, false)]));
    }

    #[test]
    fn longer_shortcuts_hold_every_key_until_completed_or_abandoned() {
        let ctrl = KeyCode::KEY_LEFTCTRL;
        let alt = KeyCode::KEY_LEFTALT;
        let mut escape = filter(&[ctrl, alt, ESC]);
        assert_eq!(press(&mut escape, ctrl), send(&[]));
        assert_eq!(press(&mut escape, alt), send(&[]));
        assert_eq!(press(&mut escape, ESC), Filtered::Escaped(vec![]));

        let mut escape = filter(&[ctrl, alt, ESC]);
        press(&mut escape, ctrl);
        press(&mut escape, alt);
        assert_eq!(release(&mut escape, alt), send(&[(ctrl, true), (alt, true), (alt, false)]));
        assert_eq!(release(&mut escape, ctrl), send(&[(ctrl, false)]));
    }
}
