//! Prints the Thunderbolt link status as the app sees it, for diagnostics:
//!   cargo run -p osm-agent --example status
fn main() {
    let s = osm_platform_macos::thunderbolt::probe();
    println!("bridge: {:?}", s.bridge);
    for p in &s.ports {
        println!(
            "port {}: connected={} speed={:?} peer={:?}",
            p.receptacle, p.connected, p.speed, p.peer
        );
    }
    println!(
        "connected peer: {:?}",
        s.connected_peer().map(|(p, d)| (&p.receptacle, &d.name))
    );
    println!("link ready: {}", s.link_ready());
    if let Some(e) = s.hardware_error {
        println!("hardware error: {e}");
    }
}
