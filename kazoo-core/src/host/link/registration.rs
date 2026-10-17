use std::io;
use std::time::Duration;

use crate::ipc::client::HubIpcClient;
use crate::ipc::link::HubAddress;
use crate::ipc::types::RegisterMsg;

pub(super) fn connect(
    address: &HubAddress,
    registration: &RegisterMsg,
    timeout: Duration,
) -> io::Result<HubIpcClient> {
    // Called at startup on the instrument's own thread, never an audio
    // callback; a TCP connect's error already names the desk it tried.
    HubIpcClient::register_with_timeout(&address.resolve()?, registration, timeout)
}
