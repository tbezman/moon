pub mod fs;
pub mod glob;
pub mod path;
pub mod process;
pub mod regex;
pub mod test;
pub mod time;

use cached::proc_macro::cached;
use std::env;
use std::time::Duration;

#[macro_export]
macro_rules! string_vec {
    () => {{
        Vec::<String>::new()
    }};
    ($($item:expr),+ $(,)?) => {{
        vec![
            $( String::from($item), )*
        ]
    }};
}

pub fn is_ci() -> bool {
    env::var("CI").is_ok()
}

#[cached(time = 60)]
pub fn is_offline() -> bool {
    use std::net::{Shutdown, SocketAddr, TcpStream};

    let addresses = [
        // Cloudflare DNS: https://1.1.1.1/dns/
        SocketAddr::from(([1, 1, 1, 1], 53)),
        SocketAddr::from(([1, 0, 0, 1], 53)),
        // Google DNS: https://developers.google.com/speed/public-dns
        SocketAddr::from(([8, 8, 8, 8], 53)),
        SocketAddr::from(([8, 8, 4, 4], 53)),
    ];

    for address in addresses {
        if let Ok(stream) = TcpStream::connect_timeout(&address, Duration::new(3, 0)) {
            stream.shutdown(Shutdown::Both).unwrap();

            return false;
        }
    }

    true
}

pub fn is_test_env() -> bool {
    env::var("MOON_TEST").is_ok()
}
