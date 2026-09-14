use crate::{Error, Result};
use system_configuration::{
    core_foundation::{
        array::CFArray,
        base::TCFType,
        runloop::{CFRunLoop, CFRunLoopSource, CFRunLoopSourceInvalidate, kCFRunLoopCommonModes},
        string::CFString,
    },
    dynamic_store::{SCDynamicStoreBuilder, SCDynamicStoreCallBackContext},
};

pub(super) const PRIMARY_SERVICE_KEY: &str = "State:/Network/Global/IPv4";

/// Watches primary IPv4 network changes on the application's main run loop.
/// Dropping the monitor removes the subscription; an executing callback may finish. Drop it on
/// the main thread while that loop is running: SCDynamicStore does not document its cancel path
/// as safe against a callback being delivered concurrently. Dropping elsewhere stays memory-safe.
#[must_use = "dropping the monitor stops network notifications"]
pub struct NetworkServiceMonitor {
    source: CFRunLoopSource,
}

// The source stays on the main run loop. Core Foundation allows invalidation from other
// threads, and the callback is Send so its context can also be released there.
unsafe impl Send for NetworkServiceMonitor {}

impl NetworkServiceMonitor {
    /// Returns once subscribed. The application must run its main CFRunLoop to receive events.
    /// The callback runs on that loop and should return promptly; it does not apply a proxy.
    pub fn start<F: FnMut() + Send + 'static>(on_change: F) -> Result<Self> {
        let store = SCDynamicStoreBuilder::new("sysproxy-rs network monitor")
            .callback_context(SCDynamicStoreCallBackContext {
                callout: |_, _, callback: &mut F| callback(),
                info: on_change,
            })
            .build()
            .ok_or(Error::SCDynamicStore)?;
        let keys = CFArray::from_CFTypes(&[CFString::from_static_string(PRIMARY_SERVICE_KEY)]);
        if !store.set_notification_keys(&keys, &CFArray::<CFString>::from_CFTypes(&[])) {
            return Err(Error::SCDynamicStore);
        }
        let source = store
            .create_run_loop_source()
            .ok_or(Error::SCDynamicStore)?;
        CFRunLoop::get_main().add_source(&source, unsafe { kCFRunLoopCommonModes });
        Ok(Self { source })
    }
}

impl Drop for NetworkServiceMonitor {
    fn drop(&mut self) {
        unsafe { CFRunLoopSourceInvalidate(self.source.as_concrete_TypeRef()) };
    }
}

#[test]
#[allow(clippy::unwrap_used)]
fn dropping_monitor_removes_subscription_and_releases_callback() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let called = Arc::new(AtomicBool::new(false));
    let callback_state = Arc::clone(&called);
    let monitor =
        NetworkServiceMonitor::start(move || callback_state.store(true, Ordering::Relaxed))
            .unwrap();
    let source = monitor.source.clone();
    let run_loop = CFRunLoop::get_main();
    assert!(run_loop.contains_source(&source, unsafe { kCFRunLoopCommonModes }));
    assert_eq!(Arc::strong_count(&called), 2);

    drop(monitor);
    assert!(!run_loop.contains_source(&source, unsafe { kCFRunLoopCommonModes }));
    drop(source);
    assert_eq!(Arc::strong_count(&called), 1);
    assert!(!called.load(Ordering::Relaxed));
}
