#include <stdlib.h>
#include <time.h>
#include "random.h"

int randInt(int lower, int upper) {
  srand(time(NULL));
  int num = (rand() % (upper - lower + 1)) + lower;

  return num;
}