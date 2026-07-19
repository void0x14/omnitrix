//! CapabilityBroker — her ajan eylemini policy'ye göre onaylar/reddeder
use crate::persona::Capability;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    Denied(&'static str),
    RequiresApproval(&'static str),
}

#[derive(Debug, Clone)]
pub struct CapabilityBroker {
    default_cap: Arc<Capability>,
}

impl CapabilityBroker {
    pub fn new(cap: Capability) -> Self {
        Self { default_cap: Arc::new(cap) }
    }

    pub fn check_fs_read(&self, _path: &Path) -> Decision {
        if self.default_cap.fs.read { Decision::Allowed }
        else { Decision::Denied("fs.read not allowed") }
    }

    pub fn check_fs_write(&self, _path: &Path) -> Decision {
        if !self.default_cap.fs.write { return Decision::Denied("fs.write not allowed") }
        Decision::Allowed
    }

    pub fn check_fs_exec(&self, path: &Path) -> Decision {
        if self.default_cap.fs.exec_allowlist.iter().any(|p| path.starts_with(p)) {
            Decision::RequiresApproval("exec needs confirmation")
        } else {
            Decision::Denied("exec not in allowlist")
        }
    }

    pub fn check_net_http(&self, host: &str) -> Decision {
        if !self.default_cap.net.http { return Decision::Denied("net.http not allowed") }
        if self.default_cap.net.host_allowlist.is_empty() { return Decision::Allowed }
        if self.default_cap.net.host_allowlist.iter().any(|h| host.contains(h)) {
            Decision::Allowed
        } else {
            Decision::RequiresApproval("host not in allowlist")
        }
    }

    pub fn check_gui_input(&self) -> Decision {
        if self.default_cap.gui.input { Decision::Allowed }
        else { Decision::Denied("gui.input not allowed") }
    }

    pub fn check_gui_screencap(&self) -> Decision {
        if self.default_cap.gui.screencap { Decision::RequiresApproval("screencap needs approval") }
        else { Decision::Denied("gui.screencap not allowed") }
    }

    pub fn check_process_spawn(&self, program: &str) -> Decision {
        if self.default_cap.process.spawn_allowlist.iter().any(|p| p == program) {
            Decision::RequiresApproval("process spawn needs approval")
        } else {
            Decision::Denied("process not in allowlist")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::{FsCap, NetCap, GuiCap, ProcessCap};

    fn read_write_cap() -> Capability {
        Capability {
            fs: FsCap { read: true, write: true, exec_allowlist: vec!["/usr/bin".into()] },
            net: NetCap { http: false, host_allowlist: vec![] },
            gui: GuiCap { input: false, screencap: false },
            process: ProcessCap { spawn_allowlist: vec![] },
        }
    }

    fn net_cap() -> Capability {
        Capability {
            fs: FsCap { read: false, write: false, exec_allowlist: vec![] },
            net: NetCap { http: true, host_allowlist: vec!["example.com".into()] },
            gui: GuiCap { input: false, screencap: false },
            process: ProcessCap { spawn_allowlist: vec![] },
        }
    }

    #[test]
    fn fs_read_allowed() {
        let broker = CapabilityBroker::new(read_write_cap());
        assert_eq!(broker.check_fs_read(Path::new("/tmp/foo")), Decision::Allowed);
    }

    #[test]
    fn fs_read_denied() {
        let cap = Capability { fs: FsCap { read: false, write: false, exec_allowlist: vec![] }, ..Default::default() };
        let broker = CapabilityBroker::new(cap);
        assert_eq!(broker.check_fs_read(Path::new("/tmp/foo")), Decision::Denied("fs.read not allowed"));
    }

    #[test]
    fn fs_write_allowed() {
        let broker = CapabilityBroker::new(read_write_cap());
        assert_eq!(broker.check_fs_write(Path::new("/tmp/bar")), Decision::Allowed);
    }

    #[test]
    fn fs_exec_in_allowlist_requires_approval() {
        let broker = CapabilityBroker::new(read_write_cap());
        assert_eq!(broker.check_fs_exec(Path::new("/usr/bin/git")), Decision::RequiresApproval("exec needs confirmation"));
    }

    #[test]
    fn fs_exec_not_in_allowlist_denied() {
        let broker = CapabilityBroker::new(read_write_cap());
        assert_eq!(broker.check_fs_exec(Path::new("/opt/malware")), Decision::Denied("exec not in allowlist"));
    }

    #[test]
    fn net_http_allowed_when_host_in_allowlist() {
        let broker = CapabilityBroker::new(net_cap());
        assert_eq!(broker.check_net_http("api.example.com"), Decision::Allowed);
    }

    #[test]
    fn net_http_requires_approval_when_host_not_in_allowlist() {
        let broker = CapabilityBroker::new(net_cap());
        assert_eq!(broker.check_net_http("evil.com"), Decision::RequiresApproval("host not in allowlist"));
    }

    #[test]
    fn net_http_allowed_when_allowlist_empty() {
        let cap = Capability {
            fs: FsCap::default(),
            net: NetCap { http: true, host_allowlist: vec![] },
            gui: GuiCap::default(),
            process: ProcessCap::default(),
        };
        let broker = CapabilityBroker::new(cap);
        assert_eq!(broker.check_net_http("any.host"), Decision::Allowed);
    }

    #[test]
    fn net_http_denied() {
        let cap = Capability {
            fs: FsCap::default(),
            net: NetCap { http: false, host_allowlist: vec![] },
            gui: GuiCap::default(),
            process: ProcessCap::default(),
        };
        let broker = CapabilityBroker::new(cap);
        assert_eq!(broker.check_net_http("any.host"), Decision::Denied("net.http not allowed"));
    }

    #[test]
    fn gui_input_denied() {
        let cap = read_write_cap();
        let broker = CapabilityBroker::new(cap);
        assert_eq!(broker.check_gui_input(), Decision::Denied("gui.input not allowed"));
    }

    #[test]
    fn gui_screencap_denied() {
        let cap = read_write_cap();
        let broker = CapabilityBroker::new(cap);
        assert_eq!(broker.check_gui_screencap(), Decision::Denied("gui.screencap not allowed"));
    }

    #[test]
    fn process_spawn_not_in_allowlist() {
        let broker = CapabilityBroker::new(read_write_cap());
        assert_eq!(broker.check_process_spawn("curl"), Decision::Denied("process not in allowlist"));
    }

    #[test]
    fn broker_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CapabilityBroker>();
    }
}
