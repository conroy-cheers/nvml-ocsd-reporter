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

You may need to reinitialise the iLO OCSD subsystem after starting this:
```bash
ssh Administrator@YOUR_ILO_ADDRESS
</>hpiLO-> ocsd reinit
```
