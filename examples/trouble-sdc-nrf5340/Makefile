OUTPUT_DIR = build
APP_TARGET_DIR = app-core/target
NET_TARGET_DIR = net-core/target
APP_ARTIFACT_DIR = $(APP_TARGET_DIR)/thumbv8m.main-none-eabihf/release
NET_ARTIFACT_DIR = $(NET_TARGET_DIR)/thumbv8m.main-none-eabi/release

.PHONY: all
all: $(OUTPUT_DIR)/merged.hex

$(OUTPUT_DIR):
	mkdir "$(OUTPUT_DIR)"

$(APP_ARTIFACT_DIR)/app-core: $(shell find ./app-core -path ./$(APP_TARGET_DIR) -prune -o -print)
	cd app-core && cargo build --release

$(OUTPUT_DIR)/app-core.elf: $(APP_ARTIFACT_DIR)/app-core | $(OUTPUT_DIR)
	cp $(APP_ARTIFACT_DIR)/app-core $@

$(NET_ARTIFACT_DIR)/net-core: $(shell find ./net-core -path ./$(NET_TARGET_DIR) -prune -o -print)
	cd net-core && cargo build --release

$(OUTPUT_DIR)/net-core.elf: $(NET_ARTIFACT_DIR)/net-core | $(OUTPUT_DIR)
	cp $(NET_ARTIFACT_DIR)/net-core $@

$(OUTPUT_DIR)/%.hex: $(OUTPUT_DIR)/%.elf
	llvm-objcopy -O ihex $^ $@

$(OUTPUT_DIR)/merged.hex: $(OUTPUT_DIR)/app-core.hex $(OUTPUT_DIR)/net-core.hex
	mergehex -o $@ -m $^

.PHONY: cleanall
cleanall:
	rm -r $(OUTPUT_DIR) $(APP_TARGET_DIR) $(NET_TARGET_DIR)

.PHONY: clean
clean:
	rm -r $(OUTPUT_DIR)

.PHONY: flash
flash: $(OUTPUT_DIR)/merged.hex
	nrfjprog --program $(OUTPUT_DIR)/merged.hex --sectorerase --verify --reset

.PHONY: flash-chiperase
flash-chiperase: $(OUTPUT_DIR)/merged.hex
	nrfjprog --program $(OUTPUT_DIR)/merged.hex --chiperase --verify --reset

.PHONY: flash-recover
flash-recover: $(OUTPUT_DIR)/merged.hex
	nrfjprog --program $(OUTPUT_DIR)/merged.hex --recover --verify --reset

.PHONY: reset
reset:
	nrfjprog --reset

.PHONY: attach-app
attach-app:
	probe-rs attach --chip nRF5340_xxAA "$(OUTPUT_DIR)/app-core.elf"

.PHONY: attach-net
attach-net:
	probe-rs attach --chip nRF5340_xxAA "$(OUTPUT_DIR)/net-core.elf"
