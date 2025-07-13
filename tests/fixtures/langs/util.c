#include <string.h>
static int check_len(const char *s);

/* copies the name */
int copy_name(char *dst, const char *src) {
    if (!check_len(src)) return -1;
    strcpy(dst, src);
    return 0;
}

static int check_len(const char *s) { return strlen(s) < 64; }
