use embassy_nrf::{
    Peripherals,
    config::{Config, HfclkSource, HfxoCapacitance, LfclkSource, LfxoCapacitance, ReghVoltage},
    gpio::{Input, Pull},
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
    // The BC40C has the hardware to support this, but I don't want to think about erratum 160
    // Hopefully erratum 166 doesn't show up, the workaround increases power usage ;-;
    config.dcdc.regh = true;
    // config.dcdc.regmain = true;
    // config.dcdc.regradio = true;
    config.hfclk_source = HfclkSource::ExternalXtal;
    config.lfclk_source = LfclkSource::ExternalXtal;
    config.internal_capacitors.hfxo = Some(HfxoCapacitance::_15_5pF);
    config.internal_capacitors.lfxo = Some(LfxoCapacitance::_7pF);
    // Configure VDD to 3.3V
    config.dcdc.regh_voltage = Some(ReghVoltage::_3V3);

    // Enable instruction cache
    pac::CACHE.enable().write(|w| w.set_enable(true));
    // Set clock to 128 MHz
    pac::CLOCK
        .hfclkctrl()
        .write(|w| w.set_hclk(pac::clock::vals::Hclk::DIV1));

    let mut peripherals = embassy_nrf::init(config);

    // probe-rs complains if network core is locked or off
    // https://github.com/probe-rs/probe-rs/issues/3053
    let mut core_peripherals = cortex_m::Peripherals::take().unwrap();
    let mut systick_delay = cortex_m::delay::Delay::new(core_peripherals.SYST, 64_000_000);
    start_network_core(&mut systick_delay);
    core_peripherals.SYST = systick_delay.free();

    {
        let hwid_b0 = Input::new(peripherals.P1_12.reborrow(), Pull::None);
        let hwid_b1 = Input::new(peripherals.P1_11.reborrow(), Pull::None);
        let hwid = ((hwid_b1.is_high() as u8) << 1) | (hwid_b0.is_high() as u8);
        defmt::info!("HWID = {=u8}", hwid);
    }

    (core_peripherals, peripherals)
}
