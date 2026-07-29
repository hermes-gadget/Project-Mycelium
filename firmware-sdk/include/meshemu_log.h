#pragma once

#include "meshemu_types.h"

#ifdef __cplusplus
extern "C" {
#endif

// Bridge firmware logging to Mycelium's tracing system.
void meshemu_log(MeshemuLogLevel level, const char* module, const char* message);

#ifdef __cplusplus
}
#endif

// Convenience macros.
#define MYCELIUM_TRACE(mod, msg) meshemu_log(MYCELIUM_LOG_TRACE, mod, msg)
#define MYCELIUM_DEBUG(mod, msg) meshemu_log(MYCELIUM_LOG_DEBUG, mod, msg)
#define MYCELIUM_INFO(mod, msg)  meshemu_log(MYCELIUM_LOG_INFO, mod, msg)
#define MYCELIUM_WARN(mod, msg)  meshemu_log(MYCELIUM_LOG_WARN, mod, msg)
#define MYCELIUM_ERROR(mod, msg) meshemu_log(MYCELIUM_LOG_ERROR, mod, msg)
