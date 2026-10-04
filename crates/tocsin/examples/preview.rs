//! Print where a message to each URL would go, without sending anything.
//!
//! ```sh
//! cargo run -p tocsin --example preview -- "ntfy://my-topic" "tgram://<bot>/<chat>"
//! ```

use tocsin::{Notification, Service};

fn main() {
    let notification = Notification::new("hello").title("preview");
    for (index, url) in std::env::args().skip(1).enumerate() {
        match url.parse::<Service>() {
            Ok(service) => {
                let requests = service.prepare(&notification);
                if requests.is_empty() {
                    println!("{service}: nothing to send");
                }
                for request in requests {
                    println!("{service}: {}", request.summary());
                }
            }
            // The error never contains the URL, so say which argument it was.
            Err(error) => eprintln!("argument {}: {error}", index + 1),
        }
    }
}
