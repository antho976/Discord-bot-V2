//! Write embeds on the dashboard and send or edit them in a channel.

use bot_core::{Feature, FeatureMeta, Group, Icon, NavItem, feature_async};

pub const KEY: &str = "embeds";

#[derive(Default)]
pub struct Embeds;

#[feature_async]
impl Feature for Embeds {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Embed builder",
            description: "Write embeds on the dashboard and send or edit them in a channel.",
            group: Group::Tools,
            nav: vec![NavItem { label: "Embed builder", path: KEY, icon: Icon::Brush }],
        }
    }
}
