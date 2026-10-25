//! The device API's session layer, shared by the feeder and every client:
//! the Noise handshakes and transport, the HMAC they and the setup code are
//! built on, the setup code itself, the length-prefixed frames that carry
//! them, and the strict hex and base64 text forms.
//!
//! It knows nothing of the feeder's hardware, storage or feeding, so a host
//! client or the owner's server takes it without the firmware's SDK. It is
//! `no_std`, allocates nothing and draws no randomness: the caller passes
//! buffers and secrets in.
#![no_std]
#![deny(unsafe_code)]

/// Declare a fieldless enumeration whose variants print as fixed words.
///
/// For the UART record fields (`stage=`, `reason=`) the host tools parse:
/// `as_str` and `Display` give exactly the declared text, so a record's
/// bytes do not depend on how the firmware names the variant.
#[macro_export]
macro_rules! named_enum {
    ($(#[$meta:meta])* $vis:vis enum $name:ident { $($(#[$vmeta:meta])* $variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        $vis enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Display::fmt(self.as_str(), f)
            }
        }
    };
}

pub mod base64;
pub mod frame;
pub mod hex;
pub mod mac;
pub mod noise;
pub mod setup_code;
