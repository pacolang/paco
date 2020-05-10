#include <string.h>
#include <stdlib.h>
#include <ctype.h>
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

// endsWith checks if a str ends with a substr
int endsWith(const char *str, const char *substr) {
  int str_len = strlen(str);
  int substr_len = strlen(substr);

  return
    (str_len >= substr_len) &&
    (0 == strcmp(str + (str_len-substr_len), substr));
}

// toLower returns the given string in lower case
char* toLower(const char *str) {
  char* new_str = (char*) malloc(strlen(str));
  strcpy(new_str, str);

  for (int i = 0; new_str[i]; i++) {
    new_str[i] = tolower(new_str[i]);
  }

  return new_str;
}
