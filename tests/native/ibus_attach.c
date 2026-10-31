/* Exercise Attach completion and input lifetimes without a session bus. */
#include "../../src/daemon/desktop/ibus/client.c"

static int ptt_presses;
static int ptt_releases;
static gboolean last_context_mode;

static gboolean connection_is_closed(GDBusConnection *connection) {
    (void)connection;
    return FALSE;
}

static gboolean record_signal(GDBusConnection *connection, const gchar *destination,
                               const gchar *path, const gchar *interface,
                               const gchar *name, GVariant *parameters, GError **error) {
    (void)connection;
    (void)destination;
    (void)path;
    (void)interface;
    (void)error;
    g_variant_ref_sink(parameters);
    if (g_str_equal(name, "Ptt")) {
        gboolean pressed;
        GVariant *target;
        g_variant_get(parameters, "(bb@" PK_TARGET_TYPE ")", &pressed,
                      &last_context_mode, &target);
        g_variant_unref(target);
        if (pressed)
            ++ptt_presses;
        else
            ++ptt_releases;
    }
    g_variant_unref(parameters);
    return TRUE;
}

/* Replace only transport: the helper's focus, privacy and key logic is unchanged. */
#define g_dbus_connection_is_closed connection_is_closed
#define g_dbus_connection_emit_signal record_signal
#include "../../src/daemon/desktop/ibus/helper.c"
#undef g_dbus_connection_is_closed
#undef g_dbus_connection_emit_signal

static gboolean key_event(PkEngine *engine, guint keyval, guint modifiers) {
    return process_key(IBUS_ENGINE(engine), keyval, 0, modifiers);
}

static void no_focus_signal(IBusEngine *engine) { (void)engine; }

static void no_content_signal(IBusEngine *engine, guint purpose, guint hints) {
    (void)engine;
    (void)purpose;
    (void)hints;
}

static gboolean pass_parent_key(IBusEngine *engine, guint keyval, guint keycode, guint state) {
    (void)engine;
    (void)keyval;
    (void)keycode;
    (void)state;
    return FALSE;
}

static void held_space_requires_release(void) {
    /* Base-class callbacks otherwise emit UI signals through a real bus. */
    IBusEngineClass *parent = g_type_class_ref(IBUS_TYPE_ENGINE_SIMPLE);
    parent->focus_in = parent->focus_out = no_focus_signal;
    parent->set_content_type = no_content_signal;
    parent->process_key_event = pass_parent_key;
    for (int contextual = 0; contextual <= 1; ++contextual) {
        PkHelper state = {.epoch = 1, .attachment = 1, .ready = TRUE, .ptt = TRUE,
                           .context_enabled = TRUE, .daemon = g_strdup(":1.7")};
        helper = &state;
        PkEngine *first = g_object_new(pk_engine_get_type(), "engine-name", "parakit",
                                      "object-path", "/org/parakit/First", NULL);
        PkEngine *second = g_object_new(pk_engine_get_type(), "engine-name", "parakit",
                                       "object-path", "/org/parakit/Second", NULL);
        first->enabled = second->enabled = TRUE;
        ptt_presses = ptt_releases = 0;
        guint modifiers = IBUS_CONTROL_MASK | (contextual ? IBUS_MOD1_MASK : 0);
        focus_in(IBUS_ENGINE(first));
        g_assert_false(key_event(first, IBUS_KEY_space, 0));
        g_assert_false(key_event(first, IBUS_KEY_space, 0));
        key_event(first, IBUS_KEY_Control_L, IBUS_CONTROL_MASK);
        if (contextual)
            key_event(first, IBUS_KEY_Alt_L, modifiers);
        g_assert_false(key_event(first, IBUS_KEY_space, modifiers));
        g_assert_cmpint(ptt_presses, ==, 0);
        key_event(first, IBUS_KEY_space, modifiers | IBUS_RELEASE_MASK);
        key_event(first, IBUS_KEY_space, modifiers);
        g_assert_cmpint(ptt_presses, ==, 1);
        g_assert_cmpint(last_context_mode, ==, contextual);

        focus_out(IBUS_ENGINE(first));
        g_assert_cmpint(ptt_releases, ==, 1);
        focus_in(IBUS_ENGINE(second));
        key_event(second, IBUS_KEY_Control_L, IBUS_RELEASE_MASK);
        key_event(second, IBUS_KEY_Control_L, modifiers);
        g_assert_true(key_event(second, IBUS_KEY_space, modifiers));
        g_assert_true(key_event(second, IBUS_KEY_space, modifiers));
        g_assert_cmpint(ptt_presses, ==, 1);
        key_event(second, IBUS_KEY_space, modifiers | IBUS_RELEASE_MASK);
        key_event(second, IBUS_KEY_space, modifiers);
        g_assert_cmpint(ptt_presses, ==, 2);

        content_type(IBUS_ENGINE(second), IBUS_INPUT_PURPOSE_PASSWORD, 0);
        g_assert_cmpint(ptt_releases, ==, 2);
        key_event(second, IBUS_KEY_Control_L, IBUS_RELEASE_MASK);
        key_event(second, IBUS_KEY_Control_L, modifiers);
        key_event(second, IBUS_KEY_space, modifiers);
        g_assert_cmpint(ptt_presses, ==, 2);
        content_type(IBUS_ENGINE(second), IBUS_INPUT_PURPOSE_FREE_FORM, 0);
        key_event(second, IBUS_KEY_space, modifiers);
        g_assert_cmpint(ptt_presses, ==, 2);
        key_event(second, IBUS_KEY_space, modifiers | IBUS_RELEASE_MASK);
        key_event(second, IBUS_KEY_space, modifiers);
        g_assert_cmpint(ptt_presses, ==, 3);
        key_event(second, IBUS_KEY_space, modifiers | IBUS_RELEASE_MASK);
        g_assert_cmpint(ptt_releases, ==, 3);

        key_event(second, IBUS_KEY_space, modifiers);
        g_assert_cmpint(ptt_presses, ==, 4);
        detach_daemon();
        g_assert_cmpint(ptt_releases, ==, 4);
        state.daemon = g_strdup(":1.8");
        state.ready = state.ptt = TRUE;
        key_event(second, IBUS_KEY_Control_L, IBUS_RELEASE_MASK);
        key_event(second, IBUS_KEY_Control_L, modifiers);
        key_event(second, IBUS_KEY_space, modifiers);
        g_assert_cmpint(ptt_presses, ==, 4);
        key_event(second, IBUS_KEY_space, modifiers | IBUS_RELEASE_MASK);
        key_event(second, IBUS_KEY_space, modifiers);
        g_assert_cmpint(ptt_presses, ==, 5);
        key_event(second, IBUS_KEY_space, modifiers | IBUS_RELEASE_MASK);
        g_assert_cmpint(ptt_releases, ==, 5);

        g_object_unref(second);
        g_object_unref(first);
        g_free(state.daemon);
        helper = NULL;
    }
    g_type_class_unref(parent);
}

static void observed(int kind, int connected, int helper, const PkTarget *target, void *user) {
    (void)connected;
    (void)helper;
    g_assert_cmpint(kind, ==, 0);
    g_assert_nonnull(target);
    g_assert_cmpuint(target->engine_id, ==, 7);
    ++*(int *)user;
}

static void complete(PkClient *client, GDBusConnection *connection, const gchar *owner,
                     uint64_t revision, GError *error) {
    AttachRequest *request = g_new0(AttachRequest, 1);
    *request = (AttachRequest){client, g_strdup(owner), revision};
    ++client->pending;
    GTask *task = g_task_new(connection, NULL, NULL, NULL);
    g_task_set_source_tag(task, g_dbus_connection_call);
    if (error) {
        g_task_return_error(task, error);
    } else {
        PkTarget target = {.engine_id = 7};
        GVariant *reply = g_variant_ref_sink(g_variant_new("(@" PK_TARGET_TYPE ")",
                                                          pk_target_variant(&target)));
        g_task_return_pointer(task, reply, (GDestroyNotify)g_variant_unref);
    }
    attached(G_OBJECT(connection), G_ASYNC_RESULT(task), request);
    g_assert_cmpuint(client->pending, ==, 0);
    g_object_unref(task);
}

static GError *old_helper_error(void) {
    return g_error_new_literal(G_DBUS_ERROR, G_DBUS_ERROR_UNKNOWN_METHOD,
                              "No such interface org.parakit.IBusBridge2");
}

int main(void) {
    ibus_init();
    GDBusConnection *connection = g_object_new(G_TYPE_DBUS_CONNECTION, NULL);
    int notifications = 0;
    PkClient client = {.owner = g_strdup(":1.7"), .revision = 2,
                       .callback = observed, .user = &notifications};

    complete(&client, connection, client.owner, 1, old_helper_error());
    complete(&client, connection, ":1.6", client.revision, old_helper_error());
    complete(&client, connection, client.owner, client.revision,
             g_error_new_literal(G_IO_ERROR, G_IO_ERROR_CANCELLED, "Cancelled"));
    client.stopping = TRUE;
    complete(&client, connection, client.owner, client.revision, old_helper_error());
    g_assert_false(client.attach_warned);
    client.stopping = FALSE;

    complete(&client, connection, client.owner, client.revision, old_helper_error());
    g_assert_true(client.attach_warned);
    complete(&client, connection, client.owner, client.revision, old_helper_error());
    g_assert_cmpint(notifications, ==, 0);
    g_assert_cmpuint(client.target.engine_id, ==, 0);

    complete(&client, connection, client.owner, client.revision, NULL);
    g_assert_false(client.attach_warned);
    g_assert_cmpint(notifications, ==, 1);
    g_assert_cmpuint(client.target.engine_id, ==, 7);
    complete(&client, connection, client.owner, client.revision, old_helper_error());
    g_assert_true(client.attach_warned);

    g_free(client.owner);
    g_object_unref(connection);
    held_space_requires_release();
    return 0;
}
