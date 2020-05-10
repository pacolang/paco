#include <string.h>
#include "strings.h"

// contains checks if a substr is contained inside the str
int contains(const char *str, const char *substr) {
  return strstr(str, substr) != NULL;
}

// startsWith checks if a str starts with a substr
int startsWith(const char *str, const char *substr) {
  size_t lenpre = strlen(substr),
           lenstr = strlen(str);
    return lenstr < lenpre ? 0 : memcmp(substr, str, lenpre) == 0;
}
