use std::io;
use std::thread;
use std::time::Duration;

use crate::ipc::client::HubIpcClient;
use crate::ipc::link::HubAddress;
use crate::ipc::types::RegisterMsg;

pub(super) fn connect(
    address: &HubAddress,
    registration: &RegisterMsg,
    timeout: Duration,
) -> io::Result<HubIpcClient> {
    let address = address.resolve()?;
    if !matches!(address, HubAddress::Tcp(_)) {
        return HubIpcClient::register_with_timeout(&address, registration, timeout);
    }
    // Startup registration precedes the persistent link worker, so DNS gets
    // its own worker rather than running on the instrument's calling thread.
    let registration = registration.clone();
    thread::Builder::new()
        .name("kazoo-hub-register".to_owned())
        .spawn(move || {
            HubIpcClient::register_with_timeout(&address, &registration, timeout)
                .map_err(|error| io::Error::new(error.kind(), format!("desk {address}: {error}")))
        })?
        .join()
        .map_err(|_| io::Error::other("desk registration worker panicked"))?
}
