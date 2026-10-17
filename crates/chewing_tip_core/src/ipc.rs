use scoped_error::impl_context_error;

pub mod client;
pub mod d512;
pub mod d512_client;
pub mod messages;
pub mod named_pipe;
pub(crate) mod server_trust;
pub mod values;
pub mod varlink;

impl_context_error!(pub IpcError);
