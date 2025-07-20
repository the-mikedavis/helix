use std::sync::atomic::AtomicBool;

use helix_event::register_hook;
use helix_view::{events::DocumentDidOpen, handlers::Handlers};

static DID_PROMPT_FOR_TRUST: AtomicBool = AtomicBool::new(false);

pub(super) fn register_hooks(_handlers: &Handlers) {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        todo!();
        // Ok(())
    });
}
