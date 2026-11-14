//! Mobile preview (design 7.7): a tab shown as a phone or tablet would show
//! it. The engine gives the page the device's screen, pixel ratio, touch and
//! user agent, so a site serves its mobile layout. Viewers then get frames
//! that size, which they letterbox like any other page.

use serde_json::json;

use super::engine::cdp::Cdp;
use super::service::BrowserError;

/// One device a tab can be previewed as.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preview {
    pub id: &'static str,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub user_agent: &'static str,
}

/// The devices offered. Clients name them; the SDK carries the same list.
pub const PREVIEWS: &[Preview] = &[
    Preview {
        id: "iphone",
        width: 393,
        height: 852,
        scale: 3.0,
        user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 \
                     (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
    },
    Preview {
        id: "android",
        width: 412,
        height: 915,
        scale: 2.625,
        user_agent: "Mozilla/5.0 (Linux; Android 15; Pixel 8) AppleWebKit/537.36 \
                     (KHTML, like Gecko) Chrome/157.0.0.0 Mobile Safari/537.36",
    },
    Preview {
        id: "ipad",
        width: 820,
        height: 1180,
        scale: 2.0,
        user_agent: "Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X) AppleWebKit/605.1.15 \
                     (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
    },
];

pub fn find(id: &str) -> Option<&'static Preview> {
    PREVIEWS.iter().find(|p| p.id == id)
}

/// Show the page in CDP session `session` as `preview`, or as a desktop page
/// with `desktop_ua` again. Back on the desktop, its size is the viewer's to
/// set again.
pub async fn apply(
    cdp: &Cdp,
    session: &str,
    preview: Option<&Preview>,
    desktop_ua: Option<&str>,
) -> Result<(), BrowserError> {
    let s = Some(session);
    match preview {
        Some(p) => {
            cdp.call(
                "Emulation.setDeviceMetricsOverride",
                json!({
                    "width": p.width,
                    "height": p.height,
                    "deviceScaleFactor": p.scale,
                    "mobile": true,
                }),
                s,
            )
            .await?;
            cdp.call(
                "Emulation.setTouchEmulationEnabled",
                json!({ "enabled": true, "maxTouchPoints": 5 }),
                s,
            )
            .await?;
            cdp.call(
                "Emulation.setUserAgentOverride",
                json!({ "userAgent": p.user_agent }),
                s,
            )
            .await?;
        }
        None => {
            cdp.call("Emulation.clearDeviceMetricsOverride", json!({}), s)
                .await?;
            cdp.call(
                "Emulation.setTouchEmulationEnabled",
                json!({ "enabled": false }),
                s,
            )
            .await?;
            if let Some(ua) = desktop_ua {
                cdp.call(
                    "Emulation.setUserAgentOverride",
                    json!({ "userAgent": ua }),
                    s,
                )
                .await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_preview_is_found_by_its_id_and_is_a_mobile_agent() {
        for p in PREVIEWS {
            assert_eq!(find(p.id), Some(p));
            assert!(p.user_agent.contains("Mobile"), "{}", p.id);
            assert!(!p.user_agent.contains("  "), "{}", p.id);
        }
        assert_eq!(find("fridge"), None);
    }
}
