use sysinfo::{Disks, Networks, Product};
fn main() {
    let disks = Disks::new_with_refreshed_list();
    for disk in disks.list() {
        if !disk.is_removable() && !disk.is_read_only() && disk.available_space() > 8_000_000_000 {
            println!("[{:?}] {:?}", disk.name(), disk.mount_point());
        }
    }

    println!("Product name: {:?}", Product::name());
    println!("Product family: {:?}", Product::family());
    println!("Vendor name: {:?}", Product::vendor_name());
    println!("SKU: {:?}", Product::stock_keeping_unit());

    // Network interfaces name, total data received and total data transmitted:
    let networks = Networks::new_with_refreshed_list();
    println!("=> networks:");
    for (interface_name, data) in &networks {
        println!(
            "{interface_name}: {} B (down) / {} B (up)",
            data.total_received(),
            data.total_transmitted(),
        );
        // If you want the amount of data received/transmitted since last call
        // to `Networks::refresh`, use `received`/`transmitted`.
        let networks = data.ip_networks();
        for ipnet in networks {
            println!("{}/{}", ipnet.addr, ipnet.prefix)
        }
    }
}
