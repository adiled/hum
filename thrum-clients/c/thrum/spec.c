/* thrum spec: the declared keys, read from the generated views. See
 * spec.h.
 */

#include "internal.h"

#include "spec.h"

#include <string.h>

const thrum_view *thrum_view_of(const char *chi) {
    size_t i;
    if (!chi) return NULL;
    for (i = 0; i < THRUM_VIEW_COUNT; i++) {
        if (strcmp(THRUM_VIEWS[i].chi, chi) == 0) return &THRUM_VIEWS[i];
    }
    return NULL;
}

static bool declares(const thrum_view *view, const char *key) {
    size_t i;
    for (i = 0; i < view->count; i++) {
        if (strcmp(view->fields[i].json, key) == 0) return true;
    }
    return false;
}

bool thrum_key_is_declared(const char *chi, const char *key) {
    const thrum_view *view = thrum_view_of(chi);
    if (!view || !key) return false;
    return declares(&THRUM_ENVELOPE, key) || declares(view, key);
}

size_t thrum_tone_undeclared(const cJSON *tone, const char **stray, size_t cap) {
    const cJSON *chi = cJSON_GetObjectItemCaseSensitive(tone, "chi");
    const cJSON *item;
    const thrum_view *view;
    size_t n = 0;

    if (!cJSON_IsObject(tone) || !cJSON_IsString(chi)) return 0;
    view = thrum_view_of(chi->valuestring);
    if (!view) return 0;
    cJSON_ArrayForEach(item, tone) {
        if (declares(&THRUM_ENVELOPE, item->string) || declares(view, item->string)) continue;
        if (stray && n < cap) stray[n] = item->string;
        n++;
    }
    return n;
}
