cmake_minimum_required(VERSION 3.20.0)

include(FetchContent)

# App core vars
set(APP_CORE_MANIFEST_PATH ../app-core/Cargo.toml)
set(APP_CORE_HEX_NAME app-core.hex)

# Build the net core
set(BOARD nrf5340dk/nrf5340/cpunet)
set(CMAKE_EXPORT_COMPILE_COMMANDS 1)
set_property(GLOBAL APPEND PROPERTY HEX_FILES_TO_MERGE ../${APP_CORE_HEX_NAME})

find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(ipc_service_remote)

target_sources(app PRIVATE src/main.c)


# Build the app core
FetchContent_Declare(
    Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.5
)
set(Rust_CARGO_TARGET thumbv8m.main-none-eabihf)
FetchContent_MakeAvailable(Corrosion)

corrosion_import_crate(
    MANIFEST_PATH ${APP_CORE_MANIFEST_PATH}
    NO_STD
    NO_LINKER_OVERRIDE
    LOCKED
    PROFILE release
)
add_custom_command(
    OUTPUT ${APP_CORE_HEX_NAME}
    COMMAND cp $<TARGET_PROPERTY:app-core,IMPORTED_LOCATION> $<TARGET_PROPERTY:app-core,IMPORTED_LOCATION>.elf
    COMMAND $<TARGET_PROPERTY:bintools,elfconvert_command>
            $<TARGET_PROPERTY:bintools,elfconvert_flag>
            $<TARGET_PROPERTY:bintools,elfconvert_flag_outtarget>ihex
            $<TARGET_PROPERTY:bintools,elfconvert_flag_infile>$<TARGET_PROPERTY:app-core,IMPORTED_LOCATION>
            $<TARGET_PROPERTY:bintools,elfconvert_flag_outfile>${APP_CORE_HEX_NAME}
            $<TARGET_PROPERTY:bintools,elfconvert_flag_final>
    COMMENT "Generating ${APP_CORE_HEX_NAME}"
    DEPENDS $<TARGET_PROPERTY:app-core,IMPORTED_LOCATION>
)
add_custom_target(app-core-hex DEPENDS ${APP_CORE_HEX_NAME})
