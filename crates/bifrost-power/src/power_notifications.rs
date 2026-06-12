//! OS power notification watcher.
//!
//! Callbacks only forward events to a worker channel. Slow recovery logic must
//! run outside the OS callback.

use std::sync::mpsc::Sender;

use crate::Result;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PowerEvent {
    CanSystemSleep,
    SystemWillSleep,
    SystemWillPowerOn,
    SystemHasPoweredOn,
    Unknown(u32),
}

pub struct PowerNotificationWatcher {
    #[cfg(target_os = "macos")]
    _inner: macos::MacOsPowerNotificationWatcher,
}

impl PowerNotificationWatcher {
    pub fn start(sender: Sender<PowerEvent>) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            macos::MacOsPowerNotificationWatcher::start(sender).map(|_inner| Self { _inner })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = sender;
            Err(crate::PowerError::Unsupported(
                crate::PlatformSupport::current().label(),
            ))
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{PowerEvent, Result};
    use crate::PowerError;
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use core_foundation_sys::runloop::{
        CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopRemoveSource, CFRunLoopRun,
        CFRunLoopSourceRef, CFRunLoopStop,
    };
    use std::ffi::c_void;
    use std::os::raw::{c_int, c_uint};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use tracing::{debug, info, warn};

    type IoObject = c_uint;
    type IoService = c_uint;
    type IoConnect = c_uint;
    type IoReturn = c_int;
    type IoNotificationPortRef = *mut c_void;
    type IoServiceInterestCallback = extern "C" fn(*mut c_void, IoService, u32, *mut c_void);

    const K_IO_RETURN_SUCCESS: IoReturn = 0;
    const K_IO_MESSAGE_CAN_SYSTEM_SLEEP: u32 = 0x0002_7000;
    const K_IO_MESSAGE_SYSTEM_WILL_SLEEP: u32 = 0x0002_8000;
    const K_IO_MESSAGE_SYSTEM_WILL_POWER_ON: u32 = 0x0003_0000;
    const K_IO_MESSAGE_SYSTEM_HAS_POWERED_ON: u32 = 0x0003_2000;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IORegisterForSystemPower(
            refcon: *mut c_void,
            the_port_ref: *mut IoNotificationPortRef,
            callback: Option<IoServiceInterestCallback>,
            notifier: *mut IoObject,
        ) -> IoConnect;

        fn IODeregisterForSystemPower(notifier: *mut IoObject) -> IoReturn;
        fn IOAllowPowerChange(kernel_port: IoConnect, notification_id: isize) -> IoReturn;
        fn IOServiceClose(connect: IoConnect) -> IoReturn;
        fn IONotificationPortGetRunLoopSource(notify: IoNotificationPortRef) -> CFRunLoopSourceRef;
        fn IONotificationPortDestroy(notify: IoNotificationPortRef);
    }

    struct CallbackContext {
        sender: mpsc::Sender<PowerEvent>,
        root_port: AtomicU32,
    }

    pub struct MacOsPowerNotificationWatcher {
        run_loop: Arc<Mutex<Option<usize>>>,
        join: Option<JoinHandle<()>>,
    }

    impl MacOsPowerNotificationWatcher {
        pub fn start(sender: mpsc::Sender<PowerEvent>) -> Result<Self> {
            let run_loop = Arc::new(Mutex::new(None));
            let thread_run_loop = run_loop.clone();
            let (ready_tx, ready_rx) = mpsc::channel::<std::result::Result<(), String>>();

            let join = thread::Builder::new()
                .name("bifrost-power-notifications".to_string())
                .spawn(move || run_power_notification_loop(sender, thread_run_loop, ready_tx))
                .map_err(|error| {
                    PowerError::Platform(format!(
                        "failed to spawn macOS power notification thread: {error}"
                    ))
                })?;

            match ready_rx.recv() {
                Ok(Ok(())) => Ok(Self {
                    run_loop,
                    join: Some(join),
                }),
                Ok(Err(error)) => {
                    let _ = join.join();
                    Err(PowerError::Platform(error))
                }
                Err(error) => {
                    let _ = join.join();
                    Err(PowerError::Platform(format!(
                        "macOS power notification thread exited before ready: {error}"
                    )))
                }
            }
        }
    }

    impl Drop for MacOsPowerNotificationWatcher {
        fn drop(&mut self) {
            let run_loop = self.run_loop.lock().ok().and_then(|guard| *guard);
            if let Some(run_loop) = run_loop {
                // SAFETY: captured from CFRunLoopGetCurrent on the watcher
                // thread and used only to request that the loop stops.
                unsafe { CFRunLoopStop(run_loop as _) };
            }

            if let Some(join) = self.join.take() {
                if join.join().is_err() {
                    warn!("macOS power notification watcher thread panicked during stop");
                }
            }
        }
    }

    fn run_power_notification_loop(
        sender: mpsc::Sender<PowerEvent>,
        run_loop_slot: Arc<Mutex<Option<usize>>>,
        ready_tx: mpsc::Sender<std::result::Result<(), String>>,
    ) {
        let context = Box::new(CallbackContext {
            sender,
            root_port: AtomicU32::new(0),
        });
        let context_ptr = Box::into_raw(context);
        let mut notification_port: IoNotificationPortRef = std::ptr::null_mut();
        let mut notifier: IoObject = 0;

        // SAFETY: context_ptr is a stable Box allocation, output pointers are
        // valid, and power_callback matches the expected ABI.
        let root_port = unsafe {
            IORegisterForSystemPower(
                context_ptr.cast(),
                &mut notification_port,
                Some(power_callback),
                &mut notifier,
            )
        };

        if notification_port.is_null() || root_port == 0 {
            // SAFETY: IOKit cannot call the context when registration failed.
            unsafe { drop(Box::from_raw(context_ptr)) };
            let _ = ready_tx.send(Err(
                "IORegisterForSystemPower returned a null notification port".to_string(),
            ));
            return;
        }
        // SAFETY: context_ptr is valid until the loop exits below.
        unsafe { (*context_ptr).root_port.store(root_port, Ordering::Release) };

        // SAFETY: notification_port is valid after successful registration.
        let source = unsafe { IONotificationPortGetRunLoopSource(notification_port) };
        if source.is_null() {
            unsafe {
                let _ = IODeregisterForSystemPower(&mut notifier);
                let _ = IOServiceClose(root_port);
                IONotificationPortDestroy(notification_port);
                drop(Box::from_raw(context_ptr));
            }
            let _ = ready_tx.send(Err(
                "IONotificationPortGetRunLoopSource returned null".to_string()
            ));
            return;
        }

        let mode = CFString::new("kCFRunLoopDefaultMode");
        // SAFETY: current run loop, source, and mode are valid for this thread.
        unsafe {
            let run_loop = CFRunLoopGetCurrent();
            if let Ok(mut slot) = run_loop_slot.lock() {
                *slot = Some(run_loop as usize);
            }
            CFRunLoopAddSource(run_loop, source, mode.as_concrete_TypeRef());
            info!("macOS power notification watcher started");
            let _ = ready_tx.send(Ok(()));
            CFRunLoopRun();
            CFRunLoopRemoveSource(run_loop, source, mode.as_concrete_TypeRef());
        }

        if let Ok(mut slot) = run_loop_slot.lock() {
            *slot = None;
        }
        unsafe {
            let rc = IODeregisterForSystemPower(&mut notifier);
            if rc != K_IO_RETURN_SUCCESS {
                warn!(
                    rc = format!("0x{:x}", rc as c_uint),
                    "IODeregisterForSystemPower failed"
                );
            }
            let rc = IOServiceClose(root_port);
            if rc != K_IO_RETURN_SUCCESS {
                warn!(
                    rc = format!("0x{:x}", rc as c_uint),
                    "IOServiceClose for power notification watcher failed"
                );
            }
            IONotificationPortDestroy(notification_port);
            drop(Box::from_raw(context_ptr));
        }
        info!("macOS power notification watcher stopped");
    }

    extern "C" fn power_callback(
        refcon: *mut c_void,
        service: IoService,
        message_type: u32,
        message_argument: *mut c_void,
    ) {
        if refcon.is_null() {
            return;
        }
        // SAFETY: refcon is the CallbackContext Box owned by the watcher
        // thread and lives until registration is deregistered.
        let context = unsafe { &*(refcon as *const CallbackContext) };
        let event = event_from_message(message_type);
        debug!(
            message_type = format!("0x{message_type:x}"),
            event = ?event,
            "macOS power notification received"
        );

        match event {
            PowerEvent::CanSystemSleep | PowerEvent::SystemWillSleep => {
                let root_port = context.root_port.load(Ordering::Acquire);
                let port = if root_port == 0 { service } else { root_port };
                let notification_id = message_argument as isize;
                // SAFETY: port is the root power connection and
                // notification_id is the callback token from IOKit.
                let rc = unsafe { IOAllowPowerChange(port, notification_id) };
                if rc != K_IO_RETURN_SUCCESS {
                    warn!(
                        rc = format!("0x{:x}", rc as c_uint),
                        event = ?event,
                        "IOAllowPowerChange failed"
                    );
                }
            }
            _ => {}
        }

        if context.sender.send(event).is_err() {
            warn!(event = ?event, "macOS power notification receiver is gone");
        }
    }

    fn event_from_message(message_type: u32) -> PowerEvent {
        match message_type {
            K_IO_MESSAGE_CAN_SYSTEM_SLEEP => PowerEvent::CanSystemSleep,
            K_IO_MESSAGE_SYSTEM_WILL_SLEEP => PowerEvent::SystemWillSleep,
            K_IO_MESSAGE_SYSTEM_WILL_POWER_ON => PowerEvent::SystemWillPowerOn,
            K_IO_MESSAGE_SYSTEM_HAS_POWERED_ON => PowerEvent::SystemHasPoweredOn,
            other => PowerEvent::Unknown(other),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn maps_iokit_power_messages() {
            assert_eq!(
                event_from_message(K_IO_MESSAGE_CAN_SYSTEM_SLEEP),
                PowerEvent::CanSystemSleep
            );
            assert_eq!(
                event_from_message(K_IO_MESSAGE_SYSTEM_WILL_SLEEP),
                PowerEvent::SystemWillSleep
            );
            assert_eq!(
                event_from_message(K_IO_MESSAGE_SYSTEM_WILL_POWER_ON),
                PowerEvent::SystemWillPowerOn
            );
            assert_eq!(
                event_from_message(K_IO_MESSAGE_SYSTEM_HAS_POWERED_ON),
                PowerEvent::SystemHasPoweredOn
            );
            assert_eq!(
                event_from_message(0xdead_beef),
                PowerEvent::Unknown(0xdead_beef)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_event_derives() {
        // Copy + Eq + Debug used across the codebase.
        let a = PowerEvent::SystemWillSleep;
        let b = a; // Copy
        assert_eq!(a, b);
        assert_ne!(PowerEvent::CanSystemSleep, PowerEvent::SystemWillSleep);
        assert_eq!(PowerEvent::Unknown(7), PowerEvent::Unknown(7));
        assert!(format!("{a:?}").contains("SystemWillSleep"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn start_is_unsupported_off_macos() {
        let (tx, _rx) = std::sync::mpsc::channel::<PowerEvent>();
        match PowerNotificationWatcher::start(tx) {
            Ok(_) => panic!("expected Unsupported error off macOS, got Ok"),
            Err(crate::PowerError::Unsupported(_)) => {}
            Err(other) => panic!("expected Unsupported error, got {other:?}"),
        }
    }
}
