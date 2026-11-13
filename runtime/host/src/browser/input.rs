//! A viewer's input to the tab it watches (design 7.6), as JSON text
//! messages on its stream socket. Coordinates are CSS pixels of the page:
//! the viewer maps its own pointer through the frame header first.

use serde::Deserialize;
use serde_json::{json, Value};

use super::engine::cdp::Cdp;
use super::service::BrowserError;
use super::stream::Viewport;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseAction {
    Move,
    Down,
    Up,
    Wheel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    #[default]
    None,
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    Down,
    Up,
}

/// One message from a viewer.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Mouse {
        action: MouseAction,
        x: f64,
        y: f64,
        #[serde(default)]
        button: MouseButton,
        /// Buttons held, as the DOM's `MouseEvent.buttons`.
        #[serde(default)]
        buttons: u32,
        #[serde(default)]
        click_count: u32,
        /// Alt 1, Ctrl 2, Meta 4, Shift 8.
        #[serde(default)]
        modifiers: u32,
        #[serde(default)]
        delta_x: f64,
        #[serde(default)]
        delta_y: f64,
    },
    Key {
        action: KeyAction,
        /// The DOM's `KeyboardEvent.key` and `.code`.
        key: String,
        code: String,
        /// What the key types, for a key that types something.
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        modifiers: u32,
        /// The DOM's `KeyboardEvent.keyCode`; without it Enter, Backspace
        /// and the arrows do nothing.
        #[serde(default)]
        key_code: Option<u32>,
    },
    /// Committed text, as a phone keyboard or a paste gives it.
    Text { text: String },
    /// The viewer's page size changed.
    Viewport {
        width: u32,
        height: u32,
        #[serde(default = "one")]
        scale: f64,
    },
}

fn one() -> f64 {
    1.0
}

impl Input {
    /// The viewport this message asks for, if it is a resize.
    pub fn viewport(&self) -> Option<Viewport> {
        match *self {
            Input::Viewport {
                width,
                height,
                scale,
            } => Some(Viewport {
                width,
                height,
                scale,
            }),
            _ => None,
        }
    }
}

/// The CDP call that delivers `input`, `None` for a resize.
pub fn cdp_call(input: &Input) -> Option<(&'static str, Value)> {
    match input {
        Input::Mouse {
            action,
            x,
            y,
            button,
            buttons,
            click_count,
            modifiers,
            delta_x,
            delta_y,
        } => {
            let kind = match action {
                MouseAction::Move => "mouseMoved",
                MouseAction::Down => "mousePressed",
                MouseAction::Up => "mouseReleased",
                MouseAction::Wheel => "mouseWheel",
            };
            let button = match button {
                MouseButton::None => "none",
                MouseButton::Left => "left",
                MouseButton::Middle => "middle",
                MouseButton::Right => "right",
                MouseButton::Back => "back",
                MouseButton::Forward => "forward",
            };
            let mut params = json!({
                "type": kind,
                "x": x,
                "y": y,
                "button": button,
                "buttons": buttons,
                "modifiers": modifiers,
                "clickCount": click_count,
            });
            if *action == MouseAction::Wheel {
                params["deltaX"] = json!(delta_x);
                params["deltaY"] = json!(delta_y);
            }
            Some(("Input.dispatchMouseEvent", params))
        }
        Input::Key {
            action,
            key,
            code,
            text,
            modifiers,
            key_code,
        } => {
            // Enter types a carriage return, as Chromium's own keyboard does.
            let text = text
                .clone()
                .or_else(|| (key == "Enter").then(|| "\r".to_string()))
                .filter(|_| *action == KeyAction::Down);
            let kind = match (action, &text) {
                (KeyAction::Up, _) => "keyUp",
                (KeyAction::Down, Some(_)) => "keyDown",
                (KeyAction::Down, None) => "rawKeyDown",
            };
            let mut params = json!({
                "type": kind,
                "key": key,
                "code": code,
                "modifiers": modifiers,
            });
            if let Some(text) = text {
                params["text"] = json!(text);
                params["unmodifiedText"] = json!(text);
            }
            if let Some(key_code) = key_code {
                params["windowsVirtualKeyCode"] = json!(key_code);
                params["nativeVirtualKeyCode"] = json!(key_code);
            }
            Some(("Input.dispatchKeyEvent", params))
        }
        Input::Text { text } => Some(("Input.insertText", json!({ "text": text }))),
        Input::Viewport { .. } => None,
    }
}

/// Deliver `input` to the page in `session`.
pub async fn dispatch(cdp: &Cdp, session: &str, input: &Input) -> Result<(), BrowserError> {
    if let Some((method, params)) = cdp_call(input) {
        cdp.call(method, params, Some(session)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: Value) -> Input {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_click_becomes_press_and_release() {
        let down = parse(
            json!({"type": "mouse", "action": "down", "x": 10.5, "y": 20,
            "button": "left", "buttons": 1, "click_count": 1}),
        );
        assert_eq!(
            cdp_call(&down).unwrap(),
            (
                "Input.dispatchMouseEvent",
                json!({"type": "mousePressed", "x": 10.5, "y": 20.0, "button": "left",
                    "buttons": 1, "modifiers": 0, "clickCount": 1})
            )
        );
        let up =
            parse(json!({"type": "mouse", "action": "up", "x": 10.5, "y": 20, "button": "left"}));
        assert_eq!(cdp_call(&up).unwrap().1["type"], "mouseReleased");
    }

    #[test]
    fn a_wheel_carries_its_deltas() {
        let wheel =
            parse(json!({"type": "mouse", "action": "wheel", "x": 1, "y": 2, "delta_y": 120}));
        let (_, params) = cdp_call(&wheel).unwrap();
        assert_eq!(params["type"], "mouseWheel");
        assert_eq!(params["deltaY"], 120.0);
        assert_eq!(params["deltaX"], 0.0);
        let moved = parse(json!({"type": "mouse", "action": "move", "x": 1, "y": 2}));
        assert!(cdp_call(&moved).unwrap().1.get("deltaY").is_none());
    }

    #[test]
    fn keys_that_type_and_keys_that_do_not() {
        let a = parse(
            json!({"type": "key", "action": "down", "key": "a", "code": "KeyA",
            "text": "a", "key_code": 65}),
        );
        let (method, params) = cdp_call(&a).unwrap();
        assert_eq!(method, "Input.dispatchKeyEvent");
        assert_eq!(params["type"], "keyDown");
        assert_eq!(params["text"], "a");
        assert_eq!(params["windowsVirtualKeyCode"], 65);

        let left = parse(json!({"type": "key", "action": "down", "key": "ArrowLeft",
            "code": "ArrowLeft", "key_code": 37}));
        let (_, params) = cdp_call(&left).unwrap();
        assert_eq!(params["type"], "rawKeyDown");
        assert!(params.get("text").is_none());

        let enter = parse(
            json!({"type": "key", "action": "down", "key": "Enter", "code": "Enter", "key_code": 13}),
        );
        assert_eq!(cdp_call(&enter).unwrap().1["text"], "\r");
        let enter_up =
            parse(json!({"type": "key", "action": "up", "key": "Enter", "code": "Enter"}));
        let (_, params) = cdp_call(&enter_up).unwrap();
        assert_eq!(params["type"], "keyUp");
        assert!(params.get("text").is_none());
    }

    #[test]
    fn text_is_inserted_and_a_resize_is_no_cdp_call() {
        let text = parse(json!({"type": "text", "text": "héllo"}));
        assert_eq!(
            cdp_call(&text).unwrap(),
            ("Input.insertText", json!({"text": "héllo"}))
        );
        let resize = parse(json!({"type": "viewport", "width": 1024, "height": 700}));
        assert!(cdp_call(&resize).is_none());
        assert_eq!(
            resize.viewport(),
            Some(Viewport {
                width: 1024,
                height: 700,
                scale: 1.0
            })
        );
    }

    #[test]
    fn unknown_messages_and_fields_are_refused() {
        for bad in [
            json!({"type": "teleport"}),
            json!({"type": "text", "text": "x", "extra": 1}),
            json!({"type": "mouse", "action": "spin", "x": 0, "y": 0}),
        ] {
            assert!(serde_json::from_value::<Input>(bad).is_err());
        }
    }
}
