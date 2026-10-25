/* Every tone in thrum-core's golden fixture fits its generated view: the
 * C counterpart of the Go and Python view tests. The client itself is
 * exercised by the conformance run, against a real humd and link.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <thrum/thrum.h>

static int kind_fits(const cJSON *v, thrum_field_kind kind) {
    switch (kind) {
    case THRUM_FIELD_STRING: return cJSON_IsString(v);
    case THRUM_FIELD_INT:
    case THRUM_FIELD_UINT:
    case THRUM_FIELD_NUMBER: return cJSON_IsNumber(v);
    case THRUM_FIELD_BOOL: return cJSON_IsBool(v);
    case THRUM_FIELD_ARRAY: return cJSON_IsArray(v);
    case THRUM_FIELD_OBJECT: return cJSON_IsObject(v);
    case THRUM_FIELD_ANY: return 1;
    }
    return 0;
}

static const thrum_view *view_of(const char *chi) {
    for (size_t i = 0; i < THRUM_VIEW_COUNT; i++)
        if (strcmp(THRUM_VIEWS[i].chi, chi) == 0) return &THRUM_VIEWS[i];
    return NULL;
}

static int fits(const cJSON *tone, const thrum_view *view, int lineno) {
    int ok = 1;
    for (size_t i = 0; i < view->count; i++) {
        const thrum_field *f = &view->fields[i];
        const cJSON *v = cJSON_GetObjectItemCaseSensitive(tone, f->json);
        if (!v || cJSON_IsNull(v)) {
            if (!f->optional) {
                fprintf(stderr, "line %d: %s lacks %s\n", lineno, view->chi ? view->chi : "envelope", f->json);
                ok = 0;
            }
        } else if (!kind_fits(v, f->kind)) {
            fprintf(stderr, "line %d: %s.%s has the wrong kind\n", lineno, view->chi ? view->chi : "envelope", f->json);
            ok = 0;
        }
    }
    return ok;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s golden.ndjson\n", argv[0]);
        return 2;
    }
    FILE *in = fopen(argv[1], "r");
    if (!in) {
        perror(argv[1]);
        return 2;
    }
    char line[1 << 16];
    int n = 0, bad = 0;
    while (fgets(line, sizeof line, in)) {
        n++;
        cJSON *tone = cJSON_Parse(line);
        const cJSON *chi = cJSON_GetObjectItemCaseSensitive(tone, "chi");
        const thrum_view *view = cJSON_IsString(chi) ? view_of(chi->valuestring) : NULL;
        if (!view) {
            fprintf(stderr, "line %d: no view for its chi\n", n);
            bad++;
        } else if (!fits(tone, &THRUM_ENVELOPE, n) || !fits(tone, view, n)) {
            bad++;
        }
        cJSON_Delete(tone);
    }
    fclose(in);
    if (bad) {
        fprintf(stderr, "%d of %d golden tones do not fit their views\n", bad, n);
        return 1;
    }
    printf("%d golden tones fit their views\n", n);
    return 0;
}
