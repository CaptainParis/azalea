//! Every URL the auth flow talks to, in one place, so a host can point the
//! whole flow at a fake (tests) or a different deployment.
//!
//! The defaults are the production URLs the rest of this crate hard-codes.

/// The Microsoft, Xbox and Minecraft endpoints of the account flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthEndpoints {
    /// Microsoft OAuth device-code request (`oauth20_connect.srf`). Only an
    /// interactive login uses it.
    pub ms_device_code: String,
    /// Microsoft OAuth token endpoint (`oauth20_token.srf`): refresh-token
    /// and device-code grants.
    pub ms_token: String,
    /// Xbox Live user authentication.
    pub xbox_user: String,
    /// Xbox XSTS authorisation.
    pub xsts: String,
    /// `api.minecraftservices.com` login with the XSTS token.
    pub mc_login: String,
    /// `api.minecraftservices.com` entitlements (ownership check).
    pub mc_entitlements: String,
    /// `api.minecraftservices.com` profile.
    pub mc_profile: String,
    /// `api.minecraftservices.com` player certificates (profile keys).
    pub mc_certificates: String,
    /// Sessionserver join (client side of an online-mode login).
    pub session_join: String,
    /// Sessionserver `hasJoined` (server side).
    pub session_has_joined: String,
}

impl Default for AuthEndpoints {
    fn default() -> Self {
        Self {
            ms_device_code: "https://login.live.com/oauth20_connect.srf".into(),
            ms_token: "https://login.live.com/oauth20_token.srf".into(),
            xbox_user: "https://user.auth.xboxlive.com/user/authenticate".into(),
            xsts: "https://xsts.auth.xboxlive.com/xsts/authorize".into(),
            mc_login: "https://api.minecraftservices.com/authentication/login_with_xbox".into(),
            mc_entitlements: "https://api.minecraftservices.com/entitlements/mcstore".into(),
            mc_profile: "https://api.minecraftservices.com/minecraft/profile".into(),
            mc_certificates: "https://api.minecraftservices.com/player/certificates".into(),
            session_join: "https://sessionserver.mojang.com/session/minecraft/join".into(),
            session_has_joined: "https://sessionserver.mojang.com/session/minecraft/hasJoined"
                .into(),
        }
    }
}

impl AuthEndpoints {
    /// Every endpoint under one base URL with the production paths (a fake
    /// that serves them all, like hunt-rig's fake Mojang).
    pub fn with_base(base: &str) -> Self {
        let b = base.trim_end_matches('/');
        Self {
            ms_device_code: format!("{b}/oauth20_connect.srf"),
            ms_token: format!("{b}/oauth20_token.srf"),
            xbox_user: format!("{b}/user/authenticate"),
            xsts: format!("{b}/xsts/authorize"),
            mc_login: format!("{b}/authentication/login_with_xbox"),
            mc_entitlements: format!("{b}/entitlements/mcstore"),
            mc_profile: format!("{b}/minecraft/profile"),
            mc_certificates: format!("{b}/player/certificates"),
            session_join: format!("{b}/session/minecraft/join"),
            session_has_joined: format!("{b}/session/minecraft/hasJoined"),
        }
    }

    /// Every endpoint, for checks that none points at production.
    pub fn all(&self) -> [&str; 10] {
        [
            &self.ms_device_code,
            &self.ms_token,
            &self.xbox_user,
            &self.xsts,
            &self.mc_login,
            &self.mc_entitlements,
            &self.mc_profile,
            &self.mc_certificates,
            &self.session_join,
            &self.session_has_joined,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_keeps_production_paths() {
        let p = AuthEndpoints::default();
        let f = AuthEndpoints::with_base("http://127.0.0.1:9/");
        for (prod, fake) in p.all().iter().zip(f.all()) {
            let path = prod.splitn(4, '/').nth(3).unwrap();
            assert_eq!(fake, format!("http://127.0.0.1:9/{path}"));
        }
    }
}
