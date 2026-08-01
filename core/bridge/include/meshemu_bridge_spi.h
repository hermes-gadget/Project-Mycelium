#ifndef MESHEMU_BRIDGE_SPI_H
#define MESHEMU_BRIDGE_SPI_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Returns the device that currently owns the shared SPI bus.
 *
 * The result is 0 when the bus is idle, 1 for the display, 2 for SX1262, and
 * 3 for the SD card.
 */
uint8_t meshemu_spi_bus_owner(void);

#ifdef __cplusplus
}
#endif

#endif
