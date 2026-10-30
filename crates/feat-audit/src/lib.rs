//! Logs edits, deletes, joins, leaves, role and nickname changes, bans and timeouts to a channel.

use bot_core::{Feature, FeatureMeta, Group, Icon, NavItem, feature_async};

pub const KEY: &str = "audit";

#[derive(Default)]
pub struct Audit;

#[feature_async]
impl Feature for Audit {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Audit log",
            description: "Logs edits, deletes, joins, leaves, role and nickname changes, bans and timeouts to a channel.",
            group: Group::Safety,
            nav: vec![NavItem { label: "Audit log", path: KEY, icon: Icon::Log }],
        }
    }
}
