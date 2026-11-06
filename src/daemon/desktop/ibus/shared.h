#ifndef PARAKIT_IBUS_SHARED_H
#define PARAKIT_IBUS_SHARED_H

#include <ibus.h>
#include <stdint.h>

#define PK_COMPONENT "org.freedesktop.IBus.Parakit"
#define PK_BRIDGE_PATH "/org/parakit/IBusBridge"
#define PK_BRIDGE_INTERFACE "org.parakit.IBusBridge2"
#define PK_TARGET_TYPE "(ttttuu)"

/* Process, attachment, engine and focus identities precede input metadata. */
typedef struct {
    uint64_t helper_epoch;
    uint64_t attachment_epoch;
    uint64_t engine_id;
    uint64_t focus_generation;
    uint32_t purpose;
    uint32_t hints;
} PkTarget;

static inline GVariant *pk_target_variant(const PkTarget *target) {
    return g_variant_new(PK_TARGET_TYPE, target->helper_epoch,
                         target->attachment_epoch, target->engine_id,
                         target->focus_generation, target->purpose, target->hints);
}

static inline PkTarget pk_target_read(GVariant *value) {
    PkTarget target;
    g_variant_get(value, PK_TARGET_TYPE, &target.helper_epoch,
                  &target.attachment_epoch, &target.engine_id,
                  &target.focus_generation, &target.purpose, &target.hints);
    return target;
}

#endif
