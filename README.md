# nvml-ocsd-reporter

Userspace agent to report NVML GPU temperature to HPE OCSD.

Packaged for Debian-based distros using systemd.

## Usage
Install `cargo-deb`:
```bash
cargo install cargo-deb
```

Pull this repository and install:
```bash
git pull https://github.com/conroy-cheers/nvml-ocsd-reporter.git
cd nvml-ocsd-reporter
cargo-deb install
```

Ensure it's running:
```bash
systemctl status nvml-ocsd-reporter
```

After starting this, the iLO OCSD subsystem needs to be reset
to recognise the newly added sensors (presumably, they are normally
detected during POST, and aren't automatically detected after boot
due to lack of hotplug support). Two options for this:

by resetting management controller (works with unmodified firmware):
```bash
ipmitool mc reset warm
```

over SSH (requires ilo4_unlock):
```bash
ssh Administrator@YOUR_ILO_ADDRESS
</>hpiLO-> ocsd reinit
```
