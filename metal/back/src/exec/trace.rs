//! `VICTORY_METAL_TRACE=1`: print every command of every submitted block to
//! stderr (the decoded proto commands, `Debug` form). A debugging aid for
//! "the result is wrong and nothing failed": it shows what the front sent.

use std::sync::OnceLock;

use victory_metal_proto::stream::Decoder;

fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_METAL_TRACE").is_some_and(|v| v != "0"))
}

/// Prints the block when tracing is on; costs one cached lookup otherwise.
pub(super) fn block(block: &[u8]) {
    if !on() {
        return;
    }
    let Ok(decoder) = Decoder::new(block) else {
        eprintln!("victory-metal trace: block does not decode");
        return;
    };
    eprintln!("victory-metal trace: block of {} bytes", block.len());
    for c in decoder {
        match c {
            Ok(c) => eprintln!("  {c:?}"),
            Err(e) => eprintln!("  error {e:?}"),
        }
    }
}
