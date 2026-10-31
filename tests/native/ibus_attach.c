/* Exercise Attach completion with real GLib async replies and no session bus. */
#include "../../src/daemon/desktop/ibus/client.c"

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
    return 0;
}
