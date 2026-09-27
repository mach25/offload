// The C ABI of crates/offload-ios (ADR-0070). Two functions, nothing else.
#include <stdint.h>

/// Start the daemon on the config at `config_path`, on a thread of its own.
/// 0 started, 1 already running, 2 missing or unreadable path, 3 the config does not load.
int32_t offload_start(const char *config_path);

/// Stop the running daemon, if there is one: it drains and announces its departure.
void offload_stop(void);
