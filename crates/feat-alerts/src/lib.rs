//! Twitch live alerts, stream schedule reminders and YouTube uploads.

use bot_core::{Feature, FeatureMeta, Group, Icon, NavItem, feature_async};

pub const KEY: &str = "alerts";

#[derive(Default)]
pub struct Alerts;

#[feature_async]
impl Feature for Alerts {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Stream alerts",
            description: "Twitch live alerts, stream schedule reminders and YouTube uploads.",
            group: Group::Streams,
            nav: vec![NavItem { label: "Stream alerts", path: KEY, icon: Icon::Live }],
        }
    }
}
