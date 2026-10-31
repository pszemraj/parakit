#include "shared.h"
#include <string.h>

typedef struct {
    IBusEngineSimple parent;
    uint64_t id;
    uint64_t generation;
    guint purpose;
    guint hints;
    gboolean enabled;
    gboolean focused;
    /* Pair one consumed Space press with its repeats and eventual release. */
    gboolean space_consumed;
} PkEngine;
typedef struct { IBusEngineSimpleClass parent; } PkEngineClass;

typedef struct {
    IBusBus *bus;
    IBusFactory *factory;
    GMainLoop *loop;
    GDBusConnection *connection;
    gchar *daemon;
    guint daemon_watch;
    guint registration;
    uint64_t epoch;
    uint64_t attachment;
    uint64_t next_engine;
    gboolean ready;
    gboolean ptt;
    gboolean context_enabled;
    /* Focus/source changes must not rearm a physically held dictation chord. */
    gboolean ptt_inhibited;
    PkEngine *focused;
    PkEngine *held;
} PkHelper;

static PkHelper *helper;
G_DEFINE_TYPE(PkEngine, pk_engine, IBUS_TYPE_ENGINE_SIMPLE)

/* Preserve content metadata even when the focused field forbids dictation. */
static PkTarget current_target(void) {
    PkTarget target = {0};
    PkEngine *engine = helper->focused;
    if (!helper->daemon || !helper->ready || !engine)
        return target;
    if (!engine->enabled || !engine->focused)
        return target;
    target = (PkTarget){helper->epoch, helper->attachment, engine->id,
                        engine->generation, engine->purpose, engine->hints};
    return target;
}

static gboolean target_is_eligible(const PkTarget *target) {
    return target->engine_id && target->purpose != IBUS_INPUT_PURPOSE_PASSWORD &&
        target->purpose != IBUS_INPUT_PURPOSE_PIN &&
        !(target->hints & IBUS_INPUT_HINT_PRIVATE);
}

static void emit_signal(const gchar *name, GVariant *parameters) {
    if (helper->daemon && !g_dbus_connection_is_closed(helper->connection))
        g_dbus_connection_emit_signal(helper->connection, helper->daemon,
            PK_BRIDGE_PATH, PK_BRIDGE_INTERFACE, name, parameters, NULL);
    else
        g_variant_unref(g_variant_ref_sink(parameters));
}

static void publish_target(void) {
    PkTarget target = current_target();
    emit_signal("TargetChanged", g_variant_new("(@" PK_TARGET_TYPE ")",
                                               pk_target_variant(&target)));
}

static void release_held(void) {
    if (!helper->held)
        return;
    helper->held = NULL;
    PkTarget empty = {0};
    emit_signal("Ptt", g_variant_new("(bb@" PK_TARGET_TYPE ")", FALSE, FALSE,
                                    pk_target_variant(&empty)));
}

static void invalidate(PkEngine *engine) {
    if (engine->space_consumed &&
        (helper->focused == engine || helper->held == engine))
        helper->ptt_inhibited = TRUE;
    if (helper->held == engine)
        release_held();
    engine->space_consumed = FALSE;
    ++engine->generation;
    publish_target();
}

static void change_focus(PkEngine *engine) {
    if (helper->focused && helper->focused != engine) {
        helper->focused->focused = FALSE;
        invalidate(helper->focused);
    }
    helper->focused = engine;
}

static gboolean process_key(IBusEngine *base, guint keyval, guint keycode, guint state) {
    PkEngine *engine = (PkEngine *)base;
    gboolean release = (state & IBUS_RELEASE_MASK) != 0;
    if (keyval == IBUS_KEY_Control_L || keyval == IBUS_KEY_Control_R ||
        (release && keyval == IBUS_KEY_space) ||
        (!release && !(state & IBUS_CONTROL_MASK)))
        helper->ptt_inhibited = FALSE;
    if (keyval == IBUS_KEY_space && engine->space_consumed) {
        if (release) {
            engine->space_consumed = FALSE;
            release_held();
        }
        return TRUE;
    }
    if (helper->held == engine && release &&
        (keyval == IBUS_KEY_space || keyval == IBUS_KEY_Control_L ||
         keyval == IBUS_KEY_Control_R)) {
        release_held();
        if (keyval == IBUS_KEY_space)
            return TRUE;
    }
    guint modifiers = state & (IBUS_CONTROL_MASK | IBUS_SHIFT_MASK |
                               IBUS_MOD1_MASK | IBUS_MOD4_MASK | IBUS_MOD5_MASK |
                               IBUS_SUPER_MASK | IBUS_META_MASK | IBUS_HYPER_MASK);
    gboolean with_context = helper->context_enabled &&
        modifiers == (IBUS_CONTROL_MASK | IBUS_MOD1_MASK);
    if (helper->ptt && !release && keyval == IBUS_KEY_space &&
        (modifiers == IBUS_CONTROL_MASK || with_context) && helper->focused == engine) {
        if (helper->ptt_inhibited)
            return TRUE;
        PkTarget target = current_target();
        if (target_is_eligible(&target)) {
            engine->space_consumed = TRUE;
            if (!helper->held) {
                helper->held = engine;
                emit_signal("Ptt", g_variant_new("(bb@" PK_TARGET_TYPE ")", TRUE, with_context,
                                                pk_target_variant(&target)));
            }
            return TRUE;
        }
    }
    return IBUS_ENGINE_CLASS(pk_engine_parent_class)->process_key_event(
        base, keyval, keycode, state);
}

static void focus_in(IBusEngine *base) {
    PkEngine *engine = (PkEngine *)base;
    IBUS_ENGINE_CLASS(pk_engine_parent_class)->focus_in(base);
    change_focus(engine);
    engine->focused = TRUE;
    invalidate(engine);
}

static void focus_in_id(IBusEngine *base, const gchar *path, const gchar *client) {
    (void)path;
    if (g_strcmp0(client, "fake") == 0) {
        change_focus(NULL);
        ((PkEngine *)base)->focused = FALSE;
        invalidate((PkEngine *)base);
        return;
    }
    focus_in(base);
}

static void focus_out(IBusEngine *base) {
    ((PkEngine *)base)->focused = FALSE;
    invalidate((PkEngine *)base);
    if (helper->focused == (PkEngine *)base)
        helper->focused = NULL;
    IBUS_ENGINE_CLASS(pk_engine_parent_class)->focus_out(base);
}

static void focus_out_id(IBusEngine *base, const gchar *path) {
    (void)path;
    focus_out(base);
}

static void reset_engine(IBusEngine *base) {
    invalidate((PkEngine *)base);
    IBUS_ENGINE_CLASS(pk_engine_parent_class)->reset(base);
}

static void enable_engine(IBusEngine *base) {
    ((PkEngine *)base)->enabled = TRUE;
    IBUS_ENGINE_CLASS(pk_engine_parent_class)->enable(base);
    invalidate((PkEngine *)base);
}

static void disable_engine(IBusEngine *base) {
    ((PkEngine *)base)->enabled = FALSE;
    invalidate((PkEngine *)base);
    IBUS_ENGINE_CLASS(pk_engine_parent_class)->disable(base);
    publish_target();
}

static void content_type(IBusEngine *base, guint purpose, guint hints) {
    PkEngine *engine = (PkEngine *)base;
    engine->purpose = purpose;
    engine->hints = hints;
    invalidate(engine);
    IBUS_ENGINE_CLASS(pk_engine_parent_class)->set_content_type(base, purpose, hints);
}

static void destroy_engine(IBusObject *object) {
    ((PkEngine *)object)->focused = FALSE;
    invalidate((PkEngine *)object);
    if (helper->focused == (PkEngine *)object)
        helper->focused = NULL;
    IBUS_OBJECT_CLASS(pk_engine_parent_class)->destroy(object);
}

static void pk_engine_class_init(PkEngineClass *klass) {
    IBusEngineClass *engine = IBUS_ENGINE_CLASS(klass);
    engine->process_key_event = process_key;
    engine->focus_in = focus_in;
    engine->focus_in_id = focus_in_id;
    engine->focus_out = focus_out;
    engine->focus_out_id = focus_out_id;
    engine->reset = reset_engine;
    engine->enable = enable_engine;
    engine->disable = disable_engine;
    engine->set_content_type = content_type;
    IBUS_OBJECT_CLASS(klass)->destroy = destroy_engine;
}

static void pk_engine_init(PkEngine *engine) {
    engine->id = ++helper->next_engine;
    engine->generation = 1;
    ibus_engine_simple_add_table_by_locale(IBUS_ENGINE_SIMPLE(engine), NULL);
}

static IBusEngine *create_engine(IBusFactory *factory, const gchar *name, gpointer data) {
    (void)factory;
    (void)data;
    if (g_strcmp0(name, "parakit") != 0)
        return NULL;
    gchar *path = g_strdup_printf("/org/freedesktop/IBus/Parakit/%" G_GUINT64_FORMAT,
                                  helper->next_engine + 1);
    IBusEngine *engine = g_object_new(pk_engine_get_type(), "engine-name", name,
        "object-path", path, "connection", helper->connection,
        "has-focus-id", TRUE, NULL);
    g_free(path);
    return engine;
}

static void detach_daemon(void) {
    if (helper->focused && helper->focused->space_consumed) {
        helper->ptt_inhibited = TRUE;
        helper->focused->space_consumed = FALSE;
    }
    release_held();
    helper->ready = FALSE;
    helper->ptt = FALSE;
    ++helper->attachment;
    g_clear_pointer(&helper->daemon, g_free);
    if (helper->daemon_watch) {
        g_bus_unwatch_name(helper->daemon_watch);
        helper->daemon_watch = 0;
    }
}

static void daemon_vanished(GDBusConnection *connection, const gchar *name, gpointer data) {
    (void)connection;
    (void)name;
    (void)data;
    detach_daemon();
}

static void bridge_call(GDBusConnection *connection, const gchar *sender,
    const gchar *path, const gchar *interface, const gchar *method,
    GVariant *parameters, GDBusMethodInvocation *invocation, gpointer data) {
    (void)path;
    (void)interface;
    (void)data;
    if (g_str_equal(method, "Attach")) {
        gboolean ready, ptt, context_enabled;
        g_variant_get(parameters, "(bbb)", &ready, &ptt, &context_enabled);
        if (g_strcmp0(helper->daemon, sender) || helper->ready != ready || helper->ptt != ptt ||
            helper->context_enabled != context_enabled) {
            detach_daemon();
            helper->daemon = g_strdup(sender);
            helper->ready = ready;
            helper->ptt = ptt;
            helper->context_enabled = context_enabled;
            helper->daemon_watch = g_bus_watch_name_on_connection(connection, sender,
                G_BUS_NAME_WATCHER_FLAGS_NONE, NULL, daemon_vanished, NULL, NULL);
        }
        PkTarget target = current_target();
        g_dbus_method_invocation_return_value(invocation,
            g_variant_new("(@" PK_TARGET_TYPE ")", pk_target_variant(&target)));
        publish_target();
    } else if (g_str_equal(method, "Detach")) {
        if (g_strcmp0(helper->daemon, sender) == 0)
            detach_daemon();
        g_dbus_method_invocation_return_value(invocation, NULL);
    } else if (g_str_equal(method, "Commit")) {
        GVariant *value;
        const gchar *text;
        g_variant_get(parameters, "(@" PK_TARGET_TYPE "&s)", &value, &text);
        PkTarget expected = pk_target_read(value);
        g_variant_unref(value);
        PkTarget current = current_target();
        gboolean accepted = g_strcmp0(helper->daemon, sender) == 0 &&
            target_is_eligible(&current) && memcmp(&expected, &current, sizeof(current)) == 0;
        if (accepted)
            ibus_engine_commit_text(IBUS_ENGINE(helper->focused), ibus_text_new_from_string(text));
        g_dbus_method_invocation_return_value(invocation, g_variant_new("(b)", accepted));
    }
}

static const GDBusInterfaceVTable bridge_vtable = {bridge_call, NULL, NULL, {0}};
static const gchar bridge_xml[] =
    "<node><interface name='" PK_BRIDGE_INTERFACE "'>"
    "<method name='Attach'><arg type='b' direction='in'/><arg type='b' direction='in'/><arg type='b' direction='in'/>"
    "<arg type='" PK_TARGET_TYPE "' direction='out'/></method>"
    "<method name='Detach'/>"
    "<method name='Commit'><arg type='" PK_TARGET_TYPE "' direction='in'/>"
    "<arg type='s' direction='in'/><arg type='b' direction='out'/></method>"
    "<signal name='TargetChanged'><arg type='" PK_TARGET_TYPE "'/></signal>"
    "<signal name='Ptt'><arg type='b'/><arg type='b'/><arg type='" PK_TARGET_TYPE "'/></signal>"
    "</interface></node>";

static void disconnected(IBusBus *bus, gpointer data) {
    (void)bus;
    (void)data;
    detach_daemon();
    g_main_loop_quit(helper->loop);
}

/* Run only the input method; the ASR daemon remains independently managed. */
int parakit_ibus_run_engine(void) {
    PkHelper state = {0};
    helper = &state;
    state.epoch = ((uint64_t)g_random_int() << 32) | g_random_int();
    state.attachment = 1;
    ibus_init();
    state.bus = ibus_bus_new();
    if (!ibus_bus_is_connected(state.bus)) {
        g_object_unref(state.bus);
        helper = NULL;
        return 1;
    }
    state.connection = g_object_ref(ibus_bus_get_connection(state.bus));
    state.loop = g_main_loop_new(NULL, FALSE);
    state.factory = ibus_factory_new(state.connection);
    g_signal_connect(state.factory, "create-engine", G_CALLBACK(create_engine), NULL);
    GDBusNodeInfo *info = g_dbus_node_info_new_for_xml(bridge_xml, NULL);
    state.registration = g_dbus_connection_register_object(state.connection,
        PK_BRIDGE_PATH, info->interfaces[0], &bridge_vtable, NULL, NULL, NULL);
    g_dbus_node_info_unref(info);
    guint32 owner = ibus_bus_request_name(state.bus, PK_COMPONENT, IBUS_BUS_NAME_FLAG_DO_NOT_QUEUE);
    int result = 1;
    if (state.registration && owner == IBUS_BUS_REQUEST_NAME_REPLY_PRIMARY_OWNER) {
        g_signal_connect(state.bus, "disconnected", G_CALLBACK(disconnected), NULL);
        g_main_loop_run(state.loop);
        result = 0;
    }
    detach_daemon();
    if (state.registration)
        g_dbus_connection_unregister_object(state.connection, state.registration);
    ibus_object_destroy(IBUS_OBJECT(state.factory));
    g_object_unref(state.factory);
    g_main_loop_unref(state.loop);
    g_object_unref(state.bus);
    g_object_unref(state.connection);
    helper = NULL;
    return result;
}
