//! Scope-bound access to the already-held ownership lock for Direct children.
//! Duplicates share one flock open-file-description; closing the final reference
//! releases ownership even when the originating process terminates abruptly.
use std::cell::RefCell;
use std::fs::File;
use std::rc::{Rc, Weak};

thread_local! {
    static ACTIVE: RefCell<Vec<Weak<File>>> = const { RefCell::new(Vec::new()) };
}

pub(super) struct Scope {
    // Rc makes the registration thread-bound and owns a valid duplicate even
    // if a caller closes its original File before dropping this scope.
    file: Rc<File>,
}
impl Scope {
    pub(super) fn register(file: &File) -> std::io::Result<Self> {
        let file = Rc::new(file.try_clone()?);
        ACTIVE.with(|active| active.borrow_mut().push(Rc::downgrade(&file)));
        Ok(Self { file })
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            let own = Rc::downgrade(&self.file);
            active.borrow_mut().retain(|entry| !entry.ptr_eq(&own));
        });
    }
}
pub(super) fn clone_current() -> std::io::Result<File> {
    ACTIVE.with(|active| {
        active
            .borrow()
            .last()
            .and_then(Weak::upgrade)
            .ok_or_else(|| std::io::Error::other("No active proxy ownership lock"))?
            .try_clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs2::FileExt;
    use std::io::Read;
    use std::process::{Command, Stdio};

    fn wait_for_lock(file: &File) {
        // Parallel tests may briefly fork with a CLOEXEC duplicate before exec.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while file.try_lock_exclusive().is_err() {
            assert!(
                std::time::Instant::now() < deadline,
                "lock was not released"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn cloned_description_holds_ownership_after_original_guard_closes() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let owner = File::options()
            .read(true)
            .write(true)
            .open(file.path())
            .unwrap();
        owner.lock_exclusive().unwrap();
        let scope = Scope::register(&owner).unwrap();
        let inherited = clone_current().unwrap();
        drop(scope);
        drop(owner);
        assert!(clone_current().is_err());
        let contender = File::options()
            .read(true)
            .write(true)
            .open(file.path())
            .unwrap();
        assert!(contender.try_lock_exclusive().is_err());
        drop(inherited);
        wait_for_lock(&contender);
    }

    #[test]
    fn child_stdin_keeps_flock_after_parent_reference_closes_then_releases_at_exit() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let owner = File::options()
            .read(true)
            .write(true)
            .open(file.path())
            .unwrap();
        owner.lock_exclusive().unwrap();
        let scope = Scope::register(&owner).unwrap();
        let mut child = Command::new("/bin/sh")
            .args(["-c", "printf ready; exec sleep 10"])
            .stdin(Stdio::from(clone_current().unwrap()))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = [0; 5];
        child.stdout.take().unwrap().read_exact(&mut ready).unwrap();
        assert_eq!(&ready, b"ready");
        drop(scope);
        drop(owner);
        let contender = File::options()
            .read(true)
            .write(true)
            .open(file.path())
            .unwrap();
        assert!(contender.try_lock_exclusive().is_err());
        child.kill().unwrap();
        child.wait().unwrap();
        wait_for_lock(&contender);
    }
    #[test]
    fn abrupt_originating_process_death_cannot_release_its_live_childs_flock() {
        struct Cleanup {
            parent: std::process::Child,
            reaped: bool,
            release: std::path::PathBuf,
        }
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::write(&self.release, b"release");
                if !self.reaped {
                    let _ = self.parent.kill();
                    let _ = self.parent.wait();
                }
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let release = directory.path().join("release-child");
        let file = tempfile::NamedTempFile::new().unwrap();
        let owner = File::options()
            .read(true)
            .write(true)
            .open(file.path())
            .unwrap();
        owner.lock_exclusive().unwrap();
        // fd9 explicitly carries the description into the grandchild. It waits
        // for a release handshake, with a bounded failsafe if the test dies.
        let script = r#"exec 9<&0
/bin/sh -c 'exec 9<&-; n=0; while [ ! -f "$1" ] && [ "$n" -lt 500 ]; do sleep 0.02; n=$((n+1)); done' lock-child "$1" <&9 &
exec 9<&-
printf ready
exec sleep 30"#;
        let parent = Command::new("/bin/sh")
            .args(["-c", script, "lock-parent"])
            .arg(&release)
            .stdin(Stdio::from(owner.try_clone().unwrap()))
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut cleanup = Cleanup {
            parent,
            reaped: false,
            release,
        };
        let mut ready = [0; 5];
        cleanup
            .parent
            .stdout
            .take()
            .unwrap()
            .read_exact(&mut ready)
            .unwrap();
        assert_eq!(&ready, b"ready");
        drop(owner);
        cleanup.parent.kill().unwrap();
        cleanup.parent.wait().unwrap();
        cleanup.reaped = true;
        let contender = File::options()
            .read(true)
            .write(true)
            .open(file.path())
            .unwrap();
        assert!(
            contender.try_lock_exclusive().is_err(),
            "origin died but its child still owns the description"
        );
        std::fs::write(&cleanup.release, b"release").unwrap();
        wait_for_lock(&contender);
    }
}
