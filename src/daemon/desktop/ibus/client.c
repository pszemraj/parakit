#include "shared.h"
#include <string.h>

typedef void (*PkCallback)(int, int, int, const PkTarget *, void *);
typedef struct {
    GMainContext *context;
    GMainLoop *loop;
    IBusBus *bus;
    GDBusConnection *connection;
    gchar *owner;
    guint owner_watch;
    guint signals;
    uint64_t revision;
    gboolean ready;
    gboolean ptt;
    gboolean held;
    gboolean stopping;
    guint pending;
    GCancellable *cancel;
    PkTarget target;
    PkCallback callback;
    void *user;
} PkClient;

static void notify(PkClient *client, int kind, const PkTarget *target) {
    client->callback(kind, client->connection != NULL, client->owner != NULL,
                     target, client->user);
}

static void clear_target(PkClient *client) {
    if (client->held) {
        client->held = FALSE;
        notify(client, 2, NULL);
    }
    memset(&client->target, 0, sizeof(client->target));
    notify(client, 0, NULL);
}

static void clear_helper(PkClient *client) {
    if (client->signals) {
        g_dbus_connection_signal_unsubscribe(client->connection, client->signals);
        client->signals = 0;
    }
    g_clear_pointer(&client->owner, g_free);
    clear_target(client);
}

typedef struct {
    PkClient *client;
    gchar *owner;
    uint64_t revision;
} AttachRequest;

static void attached(GObject *object, GAsyncResult *result, gpointer user) {
    AttachRequest *request = user;
    PkClient *client = request->client;
    GVariant *reply = g_dbus_connection_call_finish(G_DBUS_CONNECTION(object), result, NULL);
    --client->pending;
    if (!client->stopping && reply && request->revision == client->revision &&
        g_strcmp0(request->owner, client->owner) == 0) {
        GVariant *value = g_variant_get_child_value(reply, 0);
        client->target = pk_target_read(value);
        g_variant_unref(value);
        notify(client, 0, &client->target);
    }
    if (reply)
        g_variant_unref(reply);
    g_free(request->owner);
    g_free(request);
}

static void attach(PkClient *client) {
    if (client->stopping || !client->ready || !client->connection || !client->owner)
        return;
    AttachRequest *request = g_new0(AttachRequest, 1);
    request->client = client;
    request->owner = g_strdup(client->owner);
    request->revision = ++client->revision;
    ++client->pending;
    g_dbus_connection_call(client->connection, client->owner, PK_BRIDGE_PATH,
        PK_BRIDGE_INTERFACE, "Attach", g_variant_new("(bb)", client->ready, client->ptt),
        G_VARIANT_TYPE("(" PK_TARGET_TYPE ")"), G_DBUS_CALL_FLAGS_NO_AUTO_START,
        2000, client->cancel, attached, request);
}

static void bridge_signal(GDBusConnection *connection, const gchar *sender,
    const gchar *path, const gchar *interface, const gchar *name,
    GVariant *parameters, gpointer user) {
    (void)connection;
    (void)path;
    (void)interface;
    PkClient *client = user;
    if (g_strcmp0(sender, client->owner))
        return;
    GVariant *value;
    if (g_str_equal(name, "TargetChanged")) {
        value = g_variant_get_child_value(parameters, 0);
        client->target = pk_target_read(value);
        g_variant_unref(value);
        notify(client, 0, &client->target);
    } else if (g_str_equal(name, "Ptt")) {
        gboolean pressed;
        g_variant_get(parameters, "(b@" PK_TARGET_TYPE ")", &pressed, &value);
        PkTarget target = pk_target_read(value);
        g_variant_unref(value);
        if (pressed && client->ready && client->ptt && target.engine_id) {
            client->held = TRUE;
            notify(client, 1, &target);
        } else if (!pressed && client->held) {
            client->held = FALSE;
            notify(client, 2, NULL);
        }
    }
}

static void helper_appeared(GDBusConnection *connection, const gchar *name,
                            const gchar *owner, gpointer user) {
    (void)name;
    PkClient *client = user;
    clear_helper(client);
    client->owner = g_strdup(owner);
    client->signals = g_dbus_connection_signal_subscribe(connection, owner,
        PK_BRIDGE_INTERFACE, NULL, PK_BRIDGE_PATH, NULL, G_DBUS_SIGNAL_FLAGS_NONE,
        bridge_signal, client, NULL);
    notify(client, 0, NULL);
    attach(client);
}

static void helper_vanished(GDBusConnection *connection, const gchar *name, gpointer user) {
    (void)connection;
    (void)name;
    clear_helper(user);
}

static void bus_connected(IBusBus *bus, gpointer user) {
    PkClient *client = user;
    if (client->stopping)
        return;
    client->connection = g_object_ref(ibus_bus_get_connection(bus));
    client->owner_watch = g_bus_watch_name_on_connection(client->connection,
        PK_COMPONENT, G_BUS_NAME_WATCHER_FLAGS_NONE, helper_appeared, helper_vanished,
        client, NULL);
    notify(client, 0, NULL);
}

static void bus_disconnected(IBusBus *bus, gpointer user) {
    (void)bus;
    PkClient *client = user;
    if (client->owner_watch) {
        g_bus_unwatch_name(client->owner_watch);
        client->owner_watch = 0;
    }
    clear_helper(client);
    g_clear_object(&client->connection);
    notify(client, 0, NULL);
}

void *parakit_ibus_client_new(PkCallback callback, void *user) {
    PkClient *client = g_new0(PkClient, 1);
    client->context = g_main_context_new();
    client->loop = g_main_loop_new(client->context, FALSE);
    client->callback = callback;
    client->user = user;
    client->cancel = g_cancellable_new();
    return client;
}

/* Every bus object and callback belongs to this thread-default context. */
void parakit_ibus_client_run(void *pointer) {
    PkClient *client = pointer;
    g_main_context_push_thread_default(client->context);
    ibus_init();
    client->bus = ibus_bus_new();
    g_signal_connect(client->bus, "connected", G_CALLBACK(bus_connected), client);
    g_signal_connect(client->bus, "disconnected", G_CALLBACK(bus_disconnected), client);
    if (ibus_bus_is_connected(client->bus))
        bus_connected(client->bus, client);
    notify(client, 3, NULL);
    g_main_loop_run(client->loop);
    client->stopping = TRUE;
    if (client->connection && client->owner)
        g_dbus_connection_call(client->connection, client->owner, PK_BRIDGE_PATH,
            PK_BRIDGE_INTERFACE, "Detach", NULL, NULL, G_DBUS_CALL_FLAGS_NO_AUTO_START,
            500, NULL, NULL, NULL);
    if (client->owner_watch)
        g_bus_unwatch_name(client->owner_watch);
    clear_helper(client);
    g_signal_handlers_disconnect_by_data(client->bus, client);
    g_cancellable_cancel(client->cancel);
    while (client->pending)
        g_main_context_iteration(client->context, TRUE);
    ibus_object_destroy(IBUS_OBJECT(client->bus));
    g_object_unref(client->bus);
    g_clear_object(&client->connection);
    g_main_context_pop_thread_default(client->context);
}

typedef struct { PkClient *client; gboolean ready; gboolean ptt; } Configure;
static gboolean configure(gpointer user) {
    Configure *request = user;
    PkClient *client = request->client;
    if (client->ready != request->ready || client->ptt != request->ptt) {
        client->ready = request->ready;
        client->ptt = request->ptt;
        clear_target(client);
        if (client->ready) {
            attach(client);
        } else if (client->connection && client->owner) {
            ++client->revision;
            g_dbus_connection_call(client->connection, client->owner, PK_BRIDGE_PATH,
                PK_BRIDGE_INTERFACE, "Detach", NULL, NULL, G_DBUS_CALL_FLAGS_NO_AUTO_START,
                500, NULL, NULL, NULL);
        }
    }
    return G_SOURCE_REMOVE;
}

void parakit_ibus_client_configure(void *pointer, int ready, int ptt) {
    Configure *request = g_new(Configure, 1);
    *request = (Configure){pointer, ready, ptt};
    g_main_context_invoke_full(request->client->context, G_PRIORITY_DEFAULT,
                               configure, request, g_free);
}

typedef struct {
    PkClient *client;
    PkTarget expected;
    const gchar *text;
    GMutex mutex;
    GCond condition;
    gboolean done;
    int outcome;
} Commit;

static void finish_commit(Commit *request, int outcome) {
    g_mutex_lock(&request->mutex);
    request->outcome = outcome;
    request->done = TRUE;
    g_cond_signal(&request->condition);
    g_mutex_unlock(&request->mutex);
}

static void committed(GObject *object, GAsyncResult *result, gpointer user) {
    Commit *request = user;
    GVariant *reply = g_dbus_connection_call_finish(G_DBUS_CONNECTION(object), result, NULL);
    gboolean accepted = FALSE;
    if (reply) {
        g_variant_get(reply, "(b)", &accepted);
        g_variant_unref(reply);
    }
    finish_commit(request, reply ? (accepted ? 1 : 0) : 2);
}

static gboolean begin_commit(gpointer user) {
    Commit *request = user;
    PkClient *client = request->client;
    if (!client->connection || !client->owner || !client->ready ||
        !client->target.engine_id ||
        memcmp(&client->target, &request->expected, sizeof(PkTarget))) {
        finish_commit(request, 0);
        return G_SOURCE_REMOVE;
    }
    g_dbus_connection_call(client->connection, client->owner, PK_BRIDGE_PATH,
        PK_BRIDGE_INTERFACE, "Commit",
        g_variant_new("(@" PK_TARGET_TYPE "s)", pk_target_variant(&request->expected), request->text),
        G_VARIANT_TYPE("(b)"), G_DBUS_CALL_FLAGS_NO_AUTO_START, 2000, NULL,
        committed, request);
    return G_SOURCE_REMOVE;
}

/* A timeout after dispatch is uncertain; this function never retries. */
int parakit_ibus_client_commit(void *pointer, const PkTarget *target, const char *text) {
    Commit request = {.client = pointer, .expected = *target, .text = text};
    g_mutex_init(&request.mutex);
    g_cond_init(&request.condition);
    g_main_context_invoke_full(request.client->context, G_PRIORITY_DEFAULT,
                               begin_commit, &request, NULL);
    g_mutex_lock(&request.mutex);
    while (!request.done)
        g_cond_wait(&request.condition, &request.mutex);
    g_mutex_unlock(&request.mutex);
    g_cond_clear(&request.condition);
    g_mutex_clear(&request.mutex);
    return request.outcome;
}

static gboolean stop_client(gpointer user) {
    PkClient *client = user;
    g_main_loop_quit(client->loop);
    return G_SOURCE_REMOVE;
}

void parakit_ibus_client_stop(void *pointer) {
    PkClient *client = pointer;
    g_main_context_invoke_full(client->context, G_PRIORITY_DEFAULT, stop_client, client, NULL);
}

void parakit_ibus_client_free(void *pointer) {
    PkClient *client = pointer;
    g_main_loop_unref(client->loop);
    g_main_context_unref(client->context);
    g_object_unref(client->cancel);
    g_free(client);
}

/* Probe discovery without selecting an engine or attaching to the helper. */
int parakit_ibus_probe(int *listed, char **active) {
    ibus_init();
    *listed = FALSE;
    *active = NULL;
    const gchar *address = ibus_get_address();
    if (!address)
        return FALSE;
    GDBusConnection *connection = g_dbus_connection_new_for_address_sync(address,
        G_DBUS_CONNECTION_FLAGS_AUTHENTICATION_CLIENT |
        G_DBUS_CONNECTION_FLAGS_MESSAGE_BUS_CONNECTION, NULL, NULL, NULL);
    if (!connection)
        return FALSE;
    const gchar *names[] = {"parakit", NULL};
    GVariant *reply = g_dbus_connection_call_sync(connection, IBUS_SERVICE_IBUS,
        IBUS_PATH_IBUS, IBUS_INTERFACE_IBUS, "GetEnginesByNames",
        g_variant_new("(^as)", names), G_VARIANT_TYPE("(av)"),
        G_DBUS_CALL_FLAGS_NO_AUTO_START, 1000, NULL, NULL);
    if (reply) {
        GVariant *engines = g_variant_get_child_value(reply, 0);
        *listed = g_variant_n_children(engines) != 0;
        g_variant_unref(engines);
        g_variant_unref(reply);
    }
    reply = g_dbus_connection_call_sync(connection, IBUS_SERVICE_IBUS,
        IBUS_PATH_IBUS, "org.freedesktop.DBus.Properties", "Get",
        g_variant_new("(ss)", IBUS_INTERFACE_IBUS, "GlobalEngine"),
        G_VARIANT_TYPE("(v)"), G_DBUS_CALL_FLAGS_NO_AUTO_START, 1000, NULL, NULL);
    if (reply) {
        GVariant *value;
        g_variant_get(reply, "(v)", &value);
        IBusSerializable *object = ibus_serializable_deserialize(value);
        if (object && IBUS_IS_ENGINE_DESC(object))
            *active = g_strdup(ibus_engine_desc_get_name(IBUS_ENGINE_DESC(object)));
        if (object)
            g_object_unref(object);
        g_variant_unref(value);
        g_variant_unref(reply);
    }
    g_dbus_connection_close_sync(connection, NULL, NULL);
    g_object_unref(connection);
    return TRUE;
}

void parakit_ibus_string_free(char *text) { g_free(text); }
