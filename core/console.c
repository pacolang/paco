#include <stdio.h>
#include <stdlib.h>
#include "console.h"

void println(const char *message) {
  puts(message);
}

void print(const char *message) {
  printf("%s", message);
}

char* getStringEntry() {
  char* name = (char*) malloc(100);
  scanf("%s",name);

  return name;
}
