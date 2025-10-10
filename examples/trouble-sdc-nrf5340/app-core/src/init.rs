use embassy_nrf::{
    Peripherals,
    config::{Config, HfclkSource, HfxoCapacitance, LfclkSource, LfxoCapacitance},
    pac,
};

fn start_network_core(delay: &mut cortex_m::delay::Delay) {
    use embassy_nrf::pac::{self, reset::vals::Forceoff};
    // Erratum 161
    unsafe {
        (0x50005618 as *mut u32).write_volatile(1);
    }
    pac::RESET
        .network()
        .forceoff()
        .write(|w| w.set_forceoff(Forceoff::RELEASE));
    delay.delay_us(5);
    pac::RESET
        .network()
        .forceoff()
        .write(|w| w.set_forceoff(Forceoff::HOLD));
    delay.delay_us(1);
    pac::RESET
        .network()
        .forceoff()
        .write(|w| w.set_forceoff(Forceoff::RELEASE));
    unsafe {
        (0x50005618 as *mut u32).write_volatile(0);
    }
}

pub fn init() -> (cortex_m::Peripherals, Peripherals) {
    let mut config = Config::default();

    config.dcdc.regh = true;
    // Erratum 160 workaround is not implemented
    config.dcdc.regmain = false;
    config.dcdc.regradio = false;
    config.hfclk_source = HfclkSource::ExternalXtal; // HFXO is required for radio
    config.lfclk_source = LfclkSource::ExternalXtal;

    // Adjust as needed
    config.internal_capacitors.hfxo = None;
    config.internal_capacitors.lfxo = Some(LfxoCapacitance::_7pF);

    // Enable instruction cache
    pac::CACHE.enable().write(|w| w.set_enable(true));
    // Set clock to 128 MHz (avoid erratum 166)
    pac::CLOCK
        .hfclkctrl()
        .write(|w| w.set_hclk(pac::clock::vals::Hclk::DIV1));

    let mut peripherals = embassy_nrf::init(config);

    // probe-rs complains if network core is locked or off
    // https://github.com/probe-rs/probe-rs/issues/3053
    let mut core_peripherals = cortex_m::Peripherals::take().unwrap();
    let mut systick_delay = cortex_m::delay::Delay::new(core_peripherals.SYST, 128_000_000);
    start_network_core(&mut systick_delay);
    core_peripherals.SYST = systick_delay.free();
    (core_peripherals, peripherals)
}
