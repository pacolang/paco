use std::cell::RefCell;

pub type Suspend = unsafe fn(*const ());

thread_local! {
    static CONTEXTS: RefCell<Vec<(*const (), Suspend)>> = const { RefCell::new(Vec::new()) };
    static ABANDONED: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Makes the coroutine about to be resumed on this thread the one
/// [`abandon`] leaves, until the guard drops.
pub struct Isolated(());

impl Drop for Isolated {
    fn drop(&mut self) {
        CONTEXTS.with(|contexts| contexts.borrow_mut().pop());
    }
}

/// `suspend(context)` must suspend the coroutine that is resumed next.
pub fn isolate(context: *const (), suspend: Suspend) -> Isolated {
    CONTEXTS.with(|contexts| contexts.borrow_mut().push((context, suspend)));
    Isolated(())
}

/// Suspends the innermost isolated coroutine for good, leaving `message` for
/// its resumer ([`take_abandoned`]), which must discard its stack. Returns the
/// message when no coroutine is isolated on this thread.
#[inline(never)]
pub fn abandon(message: String) -> String {
    let Some((context, suspend)) = CONTEXTS.with(|contexts| contexts.borrow().last().copied()) else {
        return message;
    };
    ABANDONED.with(|abandoned| *abandoned.borrow_mut() = Some(message));
    unsafe { suspend(context) };
    unreachable!("an abandoned coroutine was resumed")
}

#[inline(never)]
pub fn take_abandoned() -> Option<String> {
    ABANDONED.with(|abandoned| abandoned.borrow_mut().take())
}
