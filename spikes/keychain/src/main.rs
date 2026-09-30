// TODO 1.8: can an **ad-hoc signed** Homebrew binary store and read a secret in
// the macOS Keychain, and does it re-prompt after every rebuild?
//
// R3 in TODO.md worries that a keychain item created by an ad-hoc-signed binary
// re-prompts on every upgrade, because the code identity changes when the binary
// is re-signed. This spike measures that instead of assuming it.
//
// The real trak binary will be `codesign -s -` (ad-hoc) and installed by Homebrew,
// so ad-hoc is the case that matters -- not a Developer ID.
//
// Usage:
//   keychain-spike store <service> <account> <secret>
//   keychain-spike read  <service> <account>
//   keychain-spike delete <service> <account>

use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (cmd, service, account) = match args.as_slice() {
        [_, c, s, a, ..] => (c.as_str(), s.as_str(), a.as_str()),
        _ => {
            eprintln!("usage: keychain-spike <store|read|delete> <service> <account> [secret]");
            std::process::exit(2);
        }
    };

    match cmd {
        "store" => {
            let secret = args
                .get(4)
                .cloned()
                .unwrap_or_else(|| "spike-secret-value-v3-yyyyyyyyyyyy".to_string());
            match set_generic_password(service, account, secret.as_bytes()) {
                Ok(()) => println!("STORED {service}/{account}"),
                Err(e) => {
                    println!("STORE-ERR {e}");
                    std::process::exit(1);
                }
            }
        }
        "read" => match get_generic_password(service, account) {
            Ok(secret) => println!(
                "READ {} bytes: {:?}",
                secret.len(),
                String::from_utf8_lossy(&secret)
            ),
            Err(e) => {
                println!("READ-ERR {e}");
                std::process::exit(1);
            }
        },
        "delete" => match delete_generic_password(service, account) {
            Ok(()) => println!("DELETED {service}/{account}"),
            Err(e) => {
                println!("DELETE-ERR {e}");
                std::process::exit(1);
            }
        },
        _ => {
            eprintln!("unknown command {cmd}");
            std::process::exit(2);
        }
    }
}
