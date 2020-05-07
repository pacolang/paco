#include <stdio.h>
#include "console.h"

void println(char *message) {
  printf("%s\n", message);
}

void print(char *message) {
  printf("%s", message);
}