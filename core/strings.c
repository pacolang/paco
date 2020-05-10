#include <string.h>
#include "strings.h"

// contains checks if a substr is contained inside the str
int contains(const char *str, const char *substr) {
  return strstr(str, substr) != NULL;
}
