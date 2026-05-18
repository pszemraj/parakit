#include <alsa/asoundlib.h>

static void parakit_drop_alsa_error(
    const char *file,
    int line,
    const char *function,
    int err,
    const char *fmt,
    ...) {
    (void)file;
    (void)line;
    (void)function;
    (void)err;
    (void)fmt;
}

void parakit_install_alsa_error_silencer(void) {
    snd_lib_error_set_handler(parakit_drop_alsa_error);
}
