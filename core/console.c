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
  char* entry = (char*) malloc(100);
  scanf("%s", entry);

  return entry;
}

int getIntEntry() {
  int entry = 0;
  scanf("%d", &entry);

  return entry;
}