#include "lcd.hpp"
#include "globals.hpp"

#include <Arduino.h>

int mainCoreId = -1;
char scrollerBuff[MAX_SCROLLER_LINE];
int scrollX = 1;
int scrollerLen = 0;
int scrollerLine = -1;
bool scrollDir = true; // true to right | false to left
unsigned long lastScrollerTime = 0;

char shownBuff[LCD_SIZE_Y][LCD_SIZE_X];
char lcdBuff[LCD_SIZE_Y][LCD_SIZE_X];
int x, y = 0;

bool lcdWriteLock = false;
bool lcdHasChanged = true;
unsigned long lcdLastDraw = 0;

inline void waitForLock() {
  while (lcdWriteLock) {
    delay(1);
  }
}

void lcdInit() {
  int coreId = xPortGetCoreID();
  mainCoreId = coreId;

  lcd.init();
  lcd.backlight();
  lcd.home();

  lcdClear();
}

void lcdLoop() {
  unsigned long timeSinceLastDraw = millis() - lcdLastDraw;
  if (scrollerLine > -1 && millis() - lastScrollerTime > SCROLLER_SPEED) {
    scrollLoop();
    lastScrollerTime = millis();
  }

  if (!lcdHasChanged || timeSinceLastDraw < 50)
    return;
  lcdHasChanged = false;

  printLcdBuff();
}

void printLcdBuff(bool force) {
  waitForLock();
  lcdWriteLock = true;

  for (int y = 0; y < LCD_SIZE_Y; y++) {
    int lastX = 0;
    lcd.setCursor(0, y);

    for (int x = 0; x < LCD_SIZE_X; x++) {
      if (/* force || */ shownBuff[y][x] != lcdBuff[y][x]) {
        if (x - lastX > 0)
          lcd.setCursor(x, y);
        lcd.print(lcdBuff[y][x]);

        shownBuff[y][x] = lcdBuff[y][x];
        lastX = x;
      }
    }
  }

  lcdLastDraw = millis();
  lcdWriteLock = false;
}

// Clears screen and sets cursor on (0, 0)
void lcdClear() {
  waitForLock();
  lcdWriteLock = true;

  memset(&lcdBuff, ' ', LCD_SIZE_X * LCD_SIZE_Y);
  // lcdBuff[LCD_SIZE_Y - 1][LCD_SIZE_X] = '\0';

  x = y = 0;
  lcdWriteLock = false;
  lcdHasChanged = true;
}

// Clears line and sets cursor at the begging of cleared line
void lcdClearLine(int line) {
  if (line < 0 || line >= LCD_SIZE_Y)
    return;

  waitForLock();
  lcdWriteLock = true;

  memset(&lcdBuff[line], ' ', LCD_SIZE_X);
  // lcdBuff[line][LCD_SIZE_X] = '\0';

  x = 0;
  y = line;

  if (line == scrollerLine) {
    scrollerLine = -1;
  }

  lcdWriteLock = false;
  lcdHasChanged = true;
}

void scrollLoop() {
  waitForLock();
  lcdWriteLock = true;

  int maxScroll = constrain(scrollerLen - 16, 0, MAX_SCROLLER_LINE - 16);

  if (scrollX <= 0)
    scrollDir = true;
  else if (scrollX >= maxScroll)
    scrollDir = false;

  char buff[LCD_SIZE_X];
  strncpy(buff, scrollerBuff + scrollX, LCD_SIZE_X);

  y = scrollerLine;
  printToScreen(buff, true, ALIGN_LEFT, true);

  if (maxScroll > 0)
    scrollX += scrollDir ? 1 : -1;
  else
    scrollX = 0;

  lcdWriteLock = false;
}

void lcdScroller(int line, const char *str) {
  waitForLock();
  lcdWriteLock = true;

  int strl = constrain(strlen(str), 0, MAX_SCROLLER_LINE);
  bool changed = line != scrollerLine;

  for (int i = 0; i < strl; i++) {
    if (scrollerBuff[i] != str[i]) {
      changed = true;
    }
  }

  if (changed) {
    scrollX = 0;
    scrollerLine = line;
    y = line;
    scrollDir = true;
    char lineBuff[LCD_SIZE_X];
    strncpy(scrollerBuff, str, strl);
    strncpy(lineBuff, str, 16);
    printToScreen(lineBuff, true, ALIGN_LEFT, true);
    scrollerLen = strl;
  }

  lcdWriteLock = false;
}

/// @brief
/// @param str string to print
/// @param fillBlank if string should be padded with spaces (blanks) to the end
/// of screen
/// @param aligment text aligment (left/center/right)
/// @param forceUnlock set to true if you dont want to lock
void printToScreen(char *str, bool fillBlank, PrintAligment aligment,
                   bool forceUnlock) {
  if (!forceUnlock) {
    waitForLock();
    lcdWriteLock = true;
  }

  int strl = strlen(str);
  int leftOffset = 0;
  switch (aligment) {
  case ALIGN_LEFT:
    leftOffset = 0;
    break;
  case ALIGN_CENTER:
    leftOffset = (LCD_SIZE_X - strl) / 2;
    break;
  case ALIGN_RIGHT:
    leftOffset = LCD_SIZE_X - strl;
    break;
  case ALIGN_NEXTTO:
    leftOffset = x;
    break;
  }

  if (leftOffset < 0)
    leftOffset = 0;
  if (fillBlank) {
    // left fill
    if (aligment != ALIGN_NEXTTO && leftOffset > 0) {
      memset(&lcdBuff[y], ' ', leftOffset);
    }

    // right fill
    int offset = leftOffset + strl;
    if (offset < LCD_SIZE_X) {
      memset(&lcdBuff[y][offset], ' ', LCD_SIZE_X - offset);
    }
  }

  strl = constrain(strl, 0, LCD_SIZE_X - leftOffset);
  strncpy(&lcdBuff[y][leftOffset], str, strl);

  x = leftOffset + strl;
  if (x >= LCD_SIZE_X)
    x = LCD_SIZE_X;

  if (!forceUnlock)
    lcdWriteLock = false;
  lcdHasChanged = true;
}

void lcdPrintf(int line, bool fillBlank, PrintAligment aligment,
               const char *format, ...) {
  if (y < 0 || y >= LCD_SIZE_Y)
    return;

  va_list arg;
  va_start(arg, format);
  char temp[64];
  char *buffer = temp;
  size_t len = vsnprintf(temp, sizeof(temp), format, arg);
  va_end(arg);
  if (len > sizeof(temp) - 1) {
    buffer = new (std::nothrow) char[len + 1];
    if (!buffer)
      return;

    va_start(arg, format);
    vsnprintf(buffer, len + 1, format, arg);
    va_end(arg);
  }

  y = line;
  if (line == scrollerLine)
    scrollerLine = -1;

  if (len > LCD_SIZE_X)
    lcdScroller(line, buffer);
  else
    printToScreen(buffer, fillBlank, aligment);

  if (buffer != temp)
    delete[] buffer;
  if (xPortGetCoreID() == mainCoreId)
    printLcdBuff();
}
