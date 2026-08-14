use anyhow::Result;
use nealsd::netns::{run_proxy_helper, PROXY_MODE_ARG};

fn main() -> Result<()> {
    // The proxy helper has to run before the tokio runtime exists: it enters the project's
    // user namespace, and setns(CLONE_NEWUSER) fails with EINVAL on a multi-threaded process.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, netns_pid, guest_port] = args.as_slice() {
        if flag == PROXY_MODE_ARG {
            return run_proxy_helper(netns_pid.parse()?, guest_port.parse()?);
        }
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(nealsd::run())
}
