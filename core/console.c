#include <stdio.h>
#include <stdlib.h>
#include "console.h"

// println prints the given message with a line break after
void println(const char *message) {
  puts(message);
}

// print prints the given message without a line break after
void print(const char *message) {
  printf("%s", message);
}

// getStringEntry returns the entered string
char* getStringEntry() {
  char* entry = (char*) malloc(100);
  scanf("%s", entry);

  return entry;
}

// getIntEntry returns the entered integer
int getIntEntry() {
  int entry = 0;
  scanf("%d", &entry);

  return entry;
}
