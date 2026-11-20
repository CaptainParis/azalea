use std::io::{self, Cursor, Write};

use azalea_buf::{AzBuf, AzBufLimited, BufReadError};
use azalea_chat::{FormattedText, style::ChatFormatting};
use azalea_protocol_macros::ClientboundGamePacket;

#[derive(AzBuf, ClientboundGamePacket, Clone, Debug, PartialEq)]
pub struct ClientboundSetPlayerTeam {
    pub name: String,
    pub method: Method,
}

#[derive(AzBuf, Clone, Debug, PartialEq)]
pub enum Method {
    Add((Parameters, PlayerList)),
    Remove,
    Change(Parameters),
    Join(PlayerList),
    Leave(PlayerList),
}

#[derive(AzBuf, Clone, Debug, PartialEq)]
pub struct Parameters {
    pub display_name: FormattedText,
    pub options: u8,
    pub nametag_visibility: NameTagVisibility,
    pub collision_rule: CollisionRule,
    /// [`ChatFormatting::Reset`] if the team has no color.
    pub color: ChatFormatting,
    pub player_prefix: FormattedText,
    pub player_suffix: FormattedText,
}

/// Defines an enum that's sent as one of vanilla's team option names (a string
/// of at most 40 characters). Unknown names are read as the first variant,
/// like the vanilla client does.
macro_rules! team_option_enum {
    ($name:ident { $($variant:ident => $id:literal),* $(,)? }) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq)]
        pub enum $name {
            #[default]
            $($variant),*
        }
        impl $name {
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $id),*
                }
            }
        }
        impl AzBuf for $name {
            fn azalea_read(buf: &mut Cursor<&[u8]>) -> Result<Self, BufReadError> {
                let name = String::azalea_read_limited(buf, 40)?;
                Ok(match name.as_str() {
                    $($id => Self::$variant,)*
                    _ => Self::default(),
                })
            }
            fn azalea_write(&self, buf: &mut impl Write) -> io::Result<()> {
                self.name().to_owned().azalea_write(buf)
            }
        }
    };
}

team_option_enum!(CollisionRule {
    Always => "always",
    Never => "never",
    PushOtherTeams => "pushOtherTeams",
    PushOwnTeam => "pushOwnTeam",
});

team_option_enum!(NameTagVisibility {
    Always => "always",
    Never => "never",
    HideForOtherTeams => "hideForOtherTeams",
    HideForOwnTeam => "hideForOwnTeam",
});

type PlayerList = Vec<String>;

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use azalea_buf::AzBuf;

    use crate::packets::game::ClientboundSetPlayerTeam;

    #[test]
    fn read_set_player_team() {
        // from hypixel, converted to the 1.21.4 field order
        #[rustfmt::skip]
        let contents = [12, 97, 56, 49, 56, 53, 51, 54, 100, 50, 98, 52, 52, 0, 10, 8, 0, 5, 99, 111, 108, 111, 114, 0, 5, 119, 104, 105, 116, 101, 8, 0, 4, 116, 101, 120, 116, 0, 12, 97, 56, 49, 56, 53, 51, 54, 100, 50, 98, 52, 52, 0, 3, 6, 97, 108, 119, 97, 121, 115, 5, 110, 101, 118, 101, 114, 10, 10, 8, 0, 4, 116, 101, 120, 116, 0, 16, 194, 167, 97, 91, 86, 73, 80, 194, 167, 54, 43, 194, 167, 97, 93, 32, 0, 10, 8, 0, 4, 116, 101, 120, 116, 0, 0, 0, 3, 10, 72, 97, 109, 97, 75, 105, 108, 108, 101, 114, 6, 90, 120, 121, 49, 111, 78, 10, 116, 97, 109, 97, 108, 101, 115, 102, 97, 109];
        let mut buf = Cursor::new(contents.as_slice());
        let _packet = ClientboundSetPlayerTeam::azalea_read(&mut buf).unwrap();

        assert_eq!(buf.position(), contents.len() as u64);
    }
}
