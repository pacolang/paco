#include <stdlib.h>
#include <time.h>
#include "random.h"

int randInt(int upper) {
  srand(time(0));
  return rand() % upper;
}