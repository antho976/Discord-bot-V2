//! Who can see a channel, worked out the way Discord does it. Features that copy a message from
//! one channel into another (the audit log, the starboard) use it to avoid showing text to
//! people who couldn't read it where it was written.

use std::collections::HashMap;

use twilight_model::channel::ChannelType;
use twilight_model::channel::permission_overwrite::{PermissionOverwrite, PermissionOverwriteType};
use twilight_model::guild::Permissions;
use twilight_model::id::Id;
use twilight_model::id::marker::GuildMarker;

use crate::Ctx;

/// What one member may do in a channel, in Discord's order: the server permissions of @everyone
/// and their roles (the owner and Administrator may do everything), then the channel's
/// overwrite for @everyone, then the denies and then the allows of all their roles' overwrites
/// together, then an overwrite for the member.
pub fn member_permissions(guild: u64, owner: bool, everyone: Permissions, role_perms: &HashMap<u64, Permissions>, roles: &[u64], user: u64, overwrites: &[PermissionOverwrite]) -> Permissions {
    let base = roles.iter().filter_map(|r| role_perms.get(r)).fold(everyone, |p, r| p | *r);
    if owner || base.contains(Permissions::ADMINISTRATOR) {
        return Permissions::all();
    }
    let mut p = base;
    if let Some(o) = overwrites.iter().find(|o| o.kind == PermissionOverwriteType::Role && o.id.get() == guild) {
        p = (p & !o.deny) | o.allow;
    }
    let (mut deny, mut allow) = (Permissions::empty(), Permissions::empty());
    for o in overwrites.iter().filter(|o| o.kind == PermissionOverwriteType::Role && roles.contains(&o.id.get())) {
        deny |= o.deny;
        allow |= o.allow;
    }
    p = (p & !deny) | allow;
    if let Some(o) = overwrites.iter().find(|o| o.kind == PermissionOverwriteType::Member && o.id.get() == user) {
        p = (p & !o.deny) | o.allow;
    }
    p
}

/// A channel as far as seeing it goes: the overwrites that decide it (a thread's come from its
/// channel), and what seeing it takes (a private thread also needs Manage Threads, for anyone
/// not added to it).
struct Seen {
    overwrites: Vec<PermissionOverwrite>,
    need: Permissions,
}

fn seen(ctx: &Ctx, channel: u64) -> Option<Seen> {
    let c = ctx.cache.channel(Id::new_checked(channel)?)?;
    if c.kind.is_thread() {
        let parent = ctx.cache.channel(c.parent_id?)?;
        let need = if c.kind == ChannelType::PrivateThread { Permissions::VIEW_CHANNEL | Permissions::MANAGE_THREADS } else { Permissions::VIEW_CHANNEL };
        Some(Seen { overwrites: parent.permission_overwrites.clone().unwrap_or_default(), need })
    } else {
        Some(Seen { overwrites: c.permission_overwrites.clone().unwrap_or_default(), need: Permissions::VIEW_CHANNEL })
    }
}

/// How long an answer of [`Ctx::readers_can_see`] is kept, by (server, reader, source).
pub(crate) type Answers = HashMap<(u64, u64, u64), (std::time::Instant, bool)>;
const KEEP: std::time::Duration = std::time::Duration::from_secs(60);

impl Ctx {
    /// Whether everyone who can read `reader` can also read `source`, so copying text from
    /// `source` into `reader` shows it to nobody new. Checked for every member the cache has
    /// (their real roles and any overwrite for them) and for a member with only @everyone, who
    /// stands in for members the cache hasn't loaded. False when the cache doesn't know either
    /// channel or the server: unknown counts as private.
    pub fn readers_can_see(&self, guild: u64, reader: u64, source: u64) -> bool {
        // Kept a minute: the starboard asks on every reaction, and a big server has thousands of
        // members to go through. Overwrites and roles rarely change faster than that matters.
        let key = (guild, reader, source);
        if let Some((at, ok)) = self.seen.lock().unwrap().get(&key)
            && at.elapsed() < KEEP
        {
            return *ok;
        }
        let ok = self.readers_can_see_now(guild, reader, source);
        let mut seen = self.seen.lock().unwrap();
        if seen.len() > 1000 {
            seen.retain(|_, (at, _)| at.elapsed() < KEEP);
        }
        seen.insert(key, (std::time::Instant::now(), ok));
        ok
    }

    fn readers_can_see_now(&self, guild: u64, reader: u64, source: u64) -> bool {
        let Some(g) = Id::<GuildMarker>::new_checked(guild) else { return false };
        let (Some(reader), Some(source)) = (seen(self, reader), seen(self, source)) else { return false };
        let Some(everyone) = self.cache.role(g.cast()).map(|r| r.permissions) else { return false };
        let owner = self.cache.guild(g).map(|x| x.owner_id().get());
        let role_perms: HashMap<u64, Permissions> = self
            .cache
            .guild_roles(g)
            .map(|set| set.iter().filter_map(|r| self.cache.role(*r).map(|role| (r.get(), role.permissions))).collect())
            .unwrap_or_default();
        let blind = |roles: &[u64], user: u64| {
            let perms = |s: &Seen| member_permissions(guild, owner == Some(user), everyone, &role_perms, roles, user, &s.overwrites);
            perms(&reader).contains(Permissions::VIEW_CHANNEL) && !perms(&source).contains(source.need)
        };
        if blind(&[], 0) {
            return false;
        }
        let members: Vec<_> = self.cache.guild_members(g).map(|set| set.iter().copied().collect()).unwrap_or_default();
        !members.into_iter().any(|u| {
            let roles: Vec<u64> = self.cache.member(g, u).map(|m| m.roles().iter().map(|r| r.get()).collect()).unwrap_or_default();
            blind(&roles, u.get())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ow(id: u64, kind: PermissionOverwriteType, allow: Permissions, deny: Permissions) -> PermissionOverwrite {
        PermissionOverwrite { allow, deny, id: Id::new(id), kind }
    }

    #[test]
    fn discords_order_decides() {
        use PermissionOverwriteType::{Member, Role};
        let view = Permissions::VIEW_CHANNEL;
        let none = Permissions::empty();
        let (guild, member, no_spoilers, mods) = (1u64, 2u64, 3u64, 4u64);
        let perms: HashMap<u64, Permissions> = [(member, view), (no_spoilers, none), (mods, Permissions::MANAGE_MESSAGES)].into();
        let sees = |roles: &[u64], user: u64, o: &[PermissionOverwrite]| member_permissions(guild, false, none, &perms, roles, user, o).contains(view);
        // A verification setup: @everyone sees nothing, Member sees everything at base.
        assert!(!sees(&[], 9, &[]));
        assert!(sees(&[member], 9, &[]));
        // A role that only denies, held with Member: the deny wins over Member's base allow.
        let spoilers = [ow(no_spoilers, Role, none, view)];
        assert!(!sees(&[member, no_spoilers], 9, &spoilers));
        // A role allow beats another role's deny on the same channel.
        let both = [ow(no_spoilers, Role, none, view), ow(mods, Role, view, none)];
        assert!(sees(&[member, no_spoilers, mods], 9, &both));
        // @everyone's overwrite comes first, then roles, then the member's own.
        let gated = [ow(guild, Role, none, view), ow(member, Role, view, none), ow(9, Member, none, view)];
        assert!(sees(&[member], 8, &gated));
        assert!(!sees(&[member], 9, &gated));
        // The owner and administrators see everything.
        assert!(member_permissions(guild, true, none, &perms, &[], 9, &gated).contains(view));
        let admin: HashMap<u64, Permissions> = [(5, Permissions::ADMINISTRATOR)].into();
        assert!(member_permissions(guild, false, none, &admin, &[5], 9, &gated).contains(view));
    }

    #[tokio::test]
    async fn readers_are_the_members_the_cache_has() {
        use twilight_model::gateway::event::Event;
        let ctx = crate::Ctx::new(crate::Config::preview(0), crate::Db::memory().unwrap()).unwrap();
        let view = 1u64 << 10;
        let role = |id: u64, perms: u64| {
            let v = serde_json::json!({"guild_id": "1", "role": {"id": id.to_string(), "name": "r", "color": 0, "colors": {"primary_color": 0}, "hoist": false, "managed": false, "mentionable": false, "permissions": perms.to_string(), "position": id, "flags": 0}});
            ctx.cache.update(&Event::RoleCreate(serde_json::from_value(v).unwrap()));
        };
        // @everyone (1) sees nothing; Member (2) sees the server; Mod (3); NoSpoilers (4).
        role(1, 0);
        role(2, view);
        role(3, 0);
        role(4, 0);
        let channel = |id: u64, ow: serde_json::Value| {
            let v = serde_json::json!({"id": id.to_string(), "type": 0, "name": "x", "guild_id": "1", "permission_overwrites": ow});
            ctx.cache.update(&Event::ChannelCreate(Box::new(serde_json::from_value(v).unwrap())));
        };
        let deny = |r: u64| serde_json::json!({"id": r.to_string(), "type": 0, "allow": "0", "deny": view.to_string()});
        let allow = |r: u64| serde_json::json!({"id": r.to_string(), "type": 0, "allow": view.to_string(), "deny": "0"});
        channel(10, serde_json::json!([]));
        channel(11, serde_json::json!([deny(1), allow(3)]));
        channel(12, serde_json::json!([deny(4)]));
        channel(13, serde_json::json!([deny(2), allow(3)]));
        let member = |user: u64, roles: &[u64]| {
            let v = serde_json::json!({"guild_id": "1", "roles": roles.iter().map(|r| r.to_string()).collect::<Vec<_>>(), "joined_at": "2025-01-01T00:00:00+00:00", "deaf": false, "mute": false, "flags": 0,
                "user": {"id": user.to_string(), "username": "u", "discriminator": "0", "avatar": null}});
            ctx.cache.update(&Event::MemberAdd(Box::new(serde_json::from_value(v).unwrap())));
        };
        member(100, &[2, 3]);
        member(101, &[2]);
        // The mods' log (11) and the general channel (10), which Member opens: mods see both.
        assert!(ctx.readers_can_see(1, 11, 10));
        // A channel only mods see (13), on a channel every member reads (10): hidden.
        assert!(!ctx.readers_can_see(1, 10, 13));
        // NoSpoilers only denies: a member holding it with Member can't read #spoilers (12).
        member(102, &[2, 4]);
        assert!(!ctx.readers_can_see(1, 10, 12));
        // Unknown channels count as private.
        assert!(!ctx.readers_can_see(1, 10, 999));
    }
}
