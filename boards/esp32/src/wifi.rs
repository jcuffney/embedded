extern crate alloc;

use alloc::string::String;

use embassy_executor::Spawner;
use embassy_net::{Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use esp_hal::peripherals::WIFI;
use esp_hal::rng::Rng;
use esp_radio::wifi::{
    Config as WifiConfig, Interface, WifiController, WifiError, sta::StationConfig,
};
use log::{info, warn};
use static_cell::StaticCell;

// embassy-net needs one socket slot per concurrent socket: DHCP client (1) +
// DNS (1) + the HTTP client's TCP connection (1) = 3. Bump this if you add
// more simultaneous connections; each slot costs a little static RAM.
const NUM_SOCKETS: usize = 3;

pub async fn start(
    spawner: Spawner,
    rng: Rng,
    wifi: WIFI<'static>,
    ssid: &'static str,
    password: &'static str,
) -> Stack<'static> {
    let mut controller = esp_radio::wifi::WifiController::new(wifi, Default::default())
        .expect("failed to create WifiController");

    let sta_conf = StationConfig::default()
        .with_ssid(ssid)
        .with_password(String::from(password));
    controller
        .set_config(&WifiConfig::Station(sta_conf))
        .expect("failed to set station config");

    let interface = Interface::station();

    static RES: StaticCell<StackResources<NUM_SOCKETS>> = StaticCell::new();
    let seed = ((rng.random() as u64) << 32) | rng.random() as u64;

    let (stack, runner) = embassy_net::new(
        interface,
        embassy_net::Config::dhcpv4(Default::default()),
        RES.init(StackResources::<NUM_SOCKETS>::new()),
        seed,
    );

    spawner.spawn(connection_task(controller).expect("connection_task token"));
    spawner.spawn(net_task(runner).expect("net_task token"));

    info!("wifi: waiting for link + DHCP");
    stack.wait_config_up().await;
    if let Some(cfg) = stack.config_v4() {
        info!(
            "wifi: got IP {} gateway {:?} dns {:?}",
            cfg.address, cfg.gateway, cfg.dns_servers
        );
    }
    stack
}

#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) {
    loop {
        match controller.connect_async().await {
            Ok(info) => {
                info!("wifi: connected to {:?}", info.ssid.as_str());
                match controller.wait_for_disconnect_async().await {
                    Ok(info) => warn!("wifi: disconnected: {:?}", info.reason),
                    Err(e) => warn!("wifi: wait_for_disconnect error: {:?}", e),
                }
            }
            Err(WifiError::Disconnected(info)) => {
                warn!("wifi: connect failed ({:?}), retrying", info.reason);
            }
            Err(e) => {
                warn!("wifi: connect error {:?}, retrying", e);
            }
        }
        Timer::after(Duration::from_secs(2)).await;
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) -> ! {
    runner.run().await
}
