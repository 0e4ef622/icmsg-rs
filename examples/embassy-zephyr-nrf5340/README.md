Building
========

This directory is intended to be a Zephyr workspace. The west manifest is at `net-core/west.yml`.

```sh
$ uv venv
$ source .venv/bin/activate
$ uv pip install west
$ west init -l net-core
$ west update --fetch-opt=--filter=tree:0  # tree-less clone to save space
$ uv pip install -r zephyr/scripts/requirements.txt
$ cd net-core
$ west build  # build the application
$ west flash build/zephyr/merged.hex  # west flash alone doesn't work for some reason
```

Output
======

Both the app core and the net core output logs over RTT.

```sh
$ probe-rs attach build/app-core
01:24:28.328: Hello, world!
01:24:28.328: MemoryConfig { send_region: 0x20070000, recv_region: 0x20078000, send_buffer_len: 2040, recv_buffer_len: 2040 }
01:24:28.328: Connected!
01:24:28.328: Sent [30]
01:24:28.328: Received 8 bytes: [0, 0, 0, 0, 0, 0, 0, 0]
01:24:28.328: Sent [30, 31]
01:24:28.328: Received 8 bytes: [1, 0, 0, 0, 0, 0, 0, 0]
01:24:28.328: Sent [30, 31, 32]
01:24:28.328: Received 8 bytes: [1, 2, 0, 0, 0, 0, 0, 0]
01:24:28.328: Sent [30, 31, 32, 33]
01:24:28.328: Received 8 bytes: [1, 2, 3, 0, 0, 0, 0, 0]
01:24:28.328: Sent [30, 31, 32, 33, 34]
01:24:28.328: Received 8 bytes: [1, 2, 3, 4, 0, 0, 0, 0]
01:24:28.328: Sent [30, 31, 32, 33, 34, 35]
01:24:28.328: Received 8 bytes: [1, 2, 3, 4, 5, 0, 0, 0]
01:24:28.328: Sent [30, 31, 32, 33, 34, 35, 36]
01:24:28.328: Received 8 bytes: [1, 2, 3, 4, 5, 6, 0, 0]
01:24:28.328: Received 8 bytes: [1, 2, 3, 4, 5, 6, 7, 0]
01:24:28.745: Received 8 bytes: [1, 2, 3, 4, 5, 6, 7, 8]
01:24:29.679: Received 8 bytes: [9, 2, 3, 4, 5, 6, 7, 8]
^C
$ probe-rs attach build/zephyr/zephyr.elf
 WARN probe_rs::rtt: Buffer for up channel 1 not initialized
 WARN probe_rs::rtt: Buffer for up channel 2 not initialized
 WARN probe_rs::rtt: Buffer for down channel 1 not initialized
 WARN probe_rs::rtt: Buffer for down channel 2 not initialized
01:24:42.231: *** Booting Zephyr OS build v4.1.0 ***
01:24:42.231: [00:00:00.000,396] <inf> remote: IPC-service REMOTE demo started
01:24:42.231: [00:00:00.000,640] <inf> remote: Ep bounded
01:24:42.231: [00:00:00.000,701] <inf> remote: Received
01:24:42.231:                                  30                                               |0
01:24:42.231: [00:00:00.000,762] <inf> remote: Perform sends for 10000 [ms]
01:24:42.231: [00:00:00.000,823] <inf> remote: Sent
01:24:42.231:                                  00 00 00 00 00 00 00 00                          |........
01:24:42.231: [00:00:01.000,732] <inf> remote: Received
01:24:42.231:                                  30 31                                            |01
01:24:42.231: [00:00:01.000,976] <inf> remote: Sent
01:24:42.231:                                  01 00 00 00 00 00 00 00                          |........
01:24:42.231: [00:00:02.000,885] <inf> remote: Received
01:24:42.231:                                  30 31 32                                         |012
01:24:42.231: [00:00:02.001,129] <inf> remote: Sent
01:24:42.231:                                  01 02 00 00 00 00 00 00                          |........
01:24:42.231: [00:00:03.000,946] <inf> remote: Received
01:24:42.231:                                  30 31 32 33                                      |0123
01:24:42.231: [00:00:03.001,281] <inf> remote: Sent
01:24:42.231:                                  01 02 03 00 00 00 00 00                          |........
01:24:42.231: [00:00:04.001,098] <inf> remote: Received
01:24:42.231:                                  30 31 32 33 34                                   |01234
01:24:42.231: [00:00:04.001,434] <inf> remote: Sent
01:24:42.231:                                  01 02 03 04 00 00 00 00                          |........
01:24:42.231: [00:00:05.001,220] <inf> remote: Received
01:24:42.231:                                  30 31 32 33 34 35                                |012345
01:24:42.231: [00:00:05.001,586] <inf> remote: Sent
01:24:42.231:                                  01 02 03 04 05 00 00 00                          |........
01:24:42.231: [00:00:06.001,403] <inf> remote: Received
01:24:42.231:                                  30 31 32 33 34 35 36                             |0123456
01:24:42.231: [00:00:06.001,739] <inf> remote: Sent
01:24:42.231:                                  01 02 03 04 05 06 00 00                          |........
01:24:42.231: [00:00:07.001,892] <inf> remote: Sent
01:24:42.231:                                  01 02 03 04 05 06 07 00                          |........
01:24:42.231: [00:00:08.002,136] <inf> remote: Sent
01:24:42.231:                                  01 02 03 04 05 06 07 08                          |........
01:24:42.231: [00:00:09.002,288] <inf> remote: Sent
01:24:42.231:                                  09 02 03 04 05 06 07 08                          |........
01:24:42.231: [00:00:10.002,502] <inf> remote: Sent 80 [Bytes] over 10000 [ms]
01:24:42.231: [00:00:10.002,502] <inf> remote: IPC-service REMOTE demo ended
```
