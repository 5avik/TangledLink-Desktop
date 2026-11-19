use std::{
    net::{TcpStream, ToSocketAddrs},
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_millis(2200);

pub fn measure(address: &str, port: u16) -> Option<u64> {
    let addresses = (address, port).to_socket_addrs().ok()?;
    let deadline = Instant::now() + TIMEOUT;
    for endpoint in addresses {
        let start = Instant::now();
        let remaining = deadline.checked_duration_since(start)?;
        if TcpStream::connect_timeout(&endpoint, remaining).is_ok() {
            return Some((start.elapsed().as_millis() as u64).max(1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn measures_a_tcp_handshake() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        assert!(measure("127.0.0.1", listener.local_addr().unwrap().port()).is_some());
    }
}
