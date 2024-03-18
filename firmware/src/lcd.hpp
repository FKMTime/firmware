#ifndef __LCD_HPP__
#define __LCD_HPP__

#define MAX_SCROLLER_LINE 64
#define SCROLLER_SPEED 500
#define SCROLLER_SETBACK 1000

#include "globals.hpp"
#include "utils.hpp"
#include "defines.h"
#include "pins.h"

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
bool blockLcd = false;
unsigned long lcdLastDraw = 0;

enum PrintAligment {
  ALIGN_LEFT = 0,
  ALIGN_CENTER = 1,
  ALIGN_RIGHT = 2,
  ALIGN_NEXTTO = 3 // ALIGN AFTER LAST TEXT
};

void printLcdBuff(bool force = false);
void lcdPrintf(int line, bool fillBlank, PrintAligment aligment, const char *format, ...);
void printToScreen(char *str, bool fillBlank, PrintAligment aligment);
void lcdClear();
void lcdClearLine(int line);
void lcdScroller(int line, const char *str);
void scrollLoop();

inline void waitForLock() {
  while(lcdWriteLock) { delay(5); }
}

void lcdInit() {
  int coreId = xPortGetCoreID();
  mainCoreId = coreId;

  lcd.init();
  lcd.backlight();
  lcd.home();

  lcdClear();
}

void lcdChange() {
  lcdHasChanged = true;
}

void lcdPrintLoop() {
  unsigned long timeSinceLastDraw = millis() - lcdLastDraw;
  if (timeSinceLastDraw > SLEEP_TIME && !lcdHasChanged) {
    lcdPrintf(0, true, ALIGN_CENTER, "Sleep mode");
    lcdPrintf(1, true, ALIGN_CENTER, "Submit to wake");
    lcd.noBacklight();
    mfrc522.PCD_SoftPowerDown();

    // enter light sleep and wait for SLEEP_WAKE_BUTTON to be pressed
    lightSleep(SLEEP_WAKE_BUTTON, LOW);

    lcd.backlight();
    lcdClear();
    lcdHasChanged = true;

    // refresh state (not done yet)

    WiFi.disconnect();
    WiFi.reconnect();
    mfrc522.PCD_SoftPowerUp();

    return;
  }

  if (scrollerLine > -1 && millis() - lastScrollerTime > SCROLLER_SPEED) {
    scrollLoop();
    lastScrollerTime = millis();
  }

  if (!lcdHasChanged || timeSinceLastDraw < 50) return;
  lcdHasChanged = false;

  printLcdBuff();
}

void printLcdBuff(bool force) {
  lcdWriteLock = true;

  for(int y = 0; y < LCD_SIZE_Y; y++) {
    for(int x = 0; x < LCD_SIZE_X; x++) {
      if(/* force || */ shownBuff[y][x] != lcdBuff[y][x]) {
        lcd.setCursor(x, y);
        lcd.print(lcdBuff[y][x]);

        shownBuff[y][x] = lcdBuff[y][x];
      }
    }
  }

  lcdLastDraw = millis();
  lcdWriteLock = false;
}

// Clears screen and sets cursor on (0, 0)
void lcdClear() {
  waitForLock();

  // TODO: C mem operation
  for (int ty = 0; ty < LCD_SIZE_Y; ty++) {
    for (int tx = 0; tx < LCD_SIZE_X; tx++) {
      lcdBuff[ty][tx] = ' ';
    }
  }

  x = y = 0;
  lcdHasChanged = true;
}

// Clears line and sets cursor at the begging of cleared line
void lcdClearLine(int line) {
  if (line < 0 || line >= LCD_SIZE_Y) return;

  waitForLock();
  // TODO: C mem operation
  for (int tx = 0; tx < LCD_SIZE_X; tx++) {
    lcdBuff[line][tx] = ' ';
  }

  x = 0;
  y = line;

  if (line == scrollerLine) {
    scrollerLine = -1;
  }
}

void scrollLoop() {
  int maxScroll = constrain(scrollerLen - 16, 0, MAX_SCROLLER_LINE - 16);

  if (scrollX <= 0) scrollDir = true;
  else if (scrollX >= maxScroll) scrollDir = false;

  char buff[LCD_SIZE_X];
  strncpy(buff, scrollerBuff + scrollX, LCD_SIZE_X);

  y = scrollerLine;
  printToScreen(buff, true, ALIGN_LEFT);

  if (maxScroll > 0) scrollX += scrollDir ? 1 : -1;
  else scrollX = 0;
}

void lcdScroller(int line, const char *str) {
  int strl = constrain(strlen(str), 0, MAX_SCROLLER_LINE);
  bool changed = line != scrollerLine;

  // TODO: C strcmpy
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
    printToScreen(lineBuff, true, ALIGN_LEFT);
    scrollerLen = strl;
  }
}

/// @brief
/// @param str string to print
/// @param fillBlank if string should be padded with spaces (blanks) to the end of screen
/// @param aligment text aligment (left/center/right)
void printToScreen(char *str, bool fillBlank = true, PrintAligment aligment = ALIGN_LEFT) {
  int strl = strlen(str);
  int leftOffset = 0;
  switch (aligment)
  {
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

  if (leftOffset < 0) leftOffset = 0;

  int strI = 0;
  waitForLock();

  // TODO: C mem operations (if fill blank, fill it first with spaces)
  for (int i = 0; i < LCD_SIZE_X; i++) {
    if (fillBlank && ((i < leftOffset && aligment != ALIGN_NEXTTO) || i >= leftOffset + strl)) {
      lcdBuff[y][i] = ' ';
      continue;
    }

    if (i >= leftOffset && strI < strl) {
      if (lcdBuff[y][i] != str[strI]) {
        lcdBuff[y][i] = str[strI];
      }

      strI++;
    }
  }

  x = leftOffset + strl;
  if (x >= LCD_SIZE_X) x = LCD_SIZE_X;
  lcdHasChanged = true;
}

void lcdPrintf(int line, bool fillBlank, PrintAligment aligment, const char *format, ...) {
  if (y < 0 || y >= LCD_SIZE_Y) return;

  va_list arg;
  va_start(arg, format);
  char temp[64];
  char *buffer = temp;
  size_t len = vsnprintf(temp, sizeof(temp), format, arg);
  va_end(arg);
  if (len > sizeof(temp) - 1) {
    buffer = new (std::nothrow) char[len + 1];
    if (!buffer) return;

    va_start(arg, format);
    vsnprintf(buffer, len + 1, format, arg);
    va_end(arg);
  }

  y = line;
  if (line == scrollerLine) scrollerLine = -1;

  if (len > LCD_SIZE_X) lcdScroller(line, buffer);
  else printToScreen(buffer, fillBlank, aligment);

  if (buffer != temp) delete[] buffer;

  if(xPortGetCoreID() == mainCoreId) printLcdBuff();
}

void blockLcdChange(bool block) {
  blockLcd = block;
}

#endif
