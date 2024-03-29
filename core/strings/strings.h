#ifndef PACO_STRINGS_H
#define PACO_STRINGS_H

int contains(const char *str, const char *substr);
int startsWith(const char *str, const char *substr);
int endsWith(const char *str, const char *substr);
char* toLower(const char *str);
char* toUpper(const char *str);

#endif //PACO_STRINGS_H
