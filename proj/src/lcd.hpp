#ifndef __LCD_HPP__
#define __LCD_HPP__

#define LCD_SIZE_X 16
#define LCD_SIZE_Y 2
#define MAX_SCROLLER_LINE 64
#define SCROLLER_SPEED 500
#define SCROLLER_SETBACK 1000

#include "globals.hpp"
#include "translations.h"

char scrollerBuff[MAX_SCROLLER_LINE];
int scrollX = 1;
int scrollerLen = 0;
int scrollerLine = -1;
bool scrollDir = true; // true to right | false to left
unsigned long lastScrollerTime = 0;

char lcdBuff[LCD_SIZE_Y][LCD_SIZE_X];
int x, y = 0;

bool lcdHasChanged = true;
unsigned long lcdLastDraw = 0;

enum PrintAligment {
  ALIGN_LEFT = 0,
  ALIGN_CENTER = 1,
  ALIGN_RIGHT = 2,
  ALIGN_NEXTTO = 3 // ALIGN AFTER LAST TEXT
};

void lcdPrintf(int line, bool fillBlank, PrintAligment aligment, const char *format, ...);
void printToScreen(char* str, bool fillBlank, PrintAligment aligment);
void lcdClearLine(int line);
void lcdScroller(int line, const char *str);
void scrollLoop();

inline void lcdInit() {
  lcd.init();
  lcd.backlight();
  lcd.home();
}

inline void lcdChange() {
  lcdHasChanged = true;
}

inline void lcdPrintLoop() {
  unsigned long timeSinceLastDraw = millis() - lcdLastDraw;
  if (timeSinceLastDraw > SLEEP_TIME && !lcdHasChanged) {

    lcdPrintf(0, true, ALIGN_CENTER, "Sleep mode");
    lcdPrintf(1, true, ALIGN_CENTER, "Submit to wake");

    lcd.noBacklight();
    sleepMode = true;

    #if defined(ESP8266)
    WiFi.forceSleepBegin(0);
    #endif
    return;
  }

  if(scrollerLine > -1 && millis() - lastScrollerTime > SCROLLER_SPEED) {
    scrollLoop();
    lastScrollerTime = millis();
  }

  if (!lcdHasChanged || timeSinceLastDraw < 50) return;
  lcdHasChanged = false;

  if (!webSocket.isConnected()) {
    lcdPrintf(0, true, ALIGN_CENTER, TR_SERVER_HEADER);
    lcdPrintf(1, true, ALIGN_CENTER, TR_DISCONNECTED);
  } else if (state.waitingForSolveResponse) {
    lcdPrintf(0, true, ALIGN_CENTER, TR_WAITING_FOR_SOLVE_TOP);
    lcdPrintf(1, true, ALIGN_CENTER, TR_WAITING_FOR_SOLVE_BOTTOM);
  } else if (state.finishedSolveTime > 0 && state.competitorCardId > 0) { // after timer is stopped and competitor scanned his card
    uint8_t minutes = state.finishedSolveTime / 60000;
    uint8_t seconds = (state.finishedSolveTime % 60000) / 1000;
    uint16_t ms = state.finishedSolveTime % 1000;

    lcdPrintf(0, true, ALIGN_LEFT, "%s", displayTime(minutes, seconds, ms).c_str());
    if(state.timeOffset == -1) {
      lcdPrintf(0, false, ALIGN_RIGHT, " DNF");
    } else if (state.timeOffset > 0) {
      lcdPrintf(0, false, ALIGN_RIGHT, " +%d", state.timeOffset);
    }
    
    if (!state.timeConfirmed) {
      lcdPrintf(1, true, ALIGN_RIGHT, TR_CONFIRM_TIME);
    } else if (state.judgeCardId == 0) {
      lcdPrintf(1, true, ALIGN_RIGHT, TR_AWAITING_JUDGE);
    } else if(state.judgeCardId > 0 && state.competitorCardId > 0) {
      lcdPrintf(1, true, ALIGN_RIGHT, TR_AWAITING_COMPETITOR_AGAIN);
    }
  } else if (!stackmat.connected()) {
    lcdPrintf(0, true, ALIGN_CENTER, TR_STACKMAT_HEADER);
    lcdPrintf(1, true, ALIGN_CENTER, TR_DISCONNECTED);
  } else if (stackmat.state() == StackmatTimerState::ST_Running && state.competitorCardId > 0) { // timer running and competitor scanned his card
    lcdPrintf(0, true, ALIGN_CENTER, "%s", displayTime(stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds()).c_str());
    lcdClearLine(1);
  } else if (state.competitorCardId > 0) {
    lcdPrintf(0, true, ALIGN_CENTER, TR_COMPETITOR);
    lcdPrintf(1, true, ALIGN_CENTER, state.competitorDisplay.c_str());
  } else if (state.competitorCardId == 0) {
    lcdPrintf(0, true, ALIGN_CENTER, TR_AWAITING_COMPETITOR_TOP);
    lcdPrintf(1, true, ALIGN_CENTER, TR_AWAITING_COMPETITOR_BOTTOM);
  } else {
    lcdPrintf(0, true, ALIGN_CENTER, TR_STACKMAT_HEADER);
    lcdPrintf(1, true, ALIGN_CENTER, TR_UNHANDLED_STATE);
  }

  lcdLastDraw = millis();
}

void restoreFromSleep() {
  #if defined(ESP8266)
  WiFi.forceSleepWake();
  #endif

  lcd.backlight();
  sleepMode = false;
  lcdHasChanged = true;
}

// Clears screen and sets cursor on (0, 0)
void lcdClear() {
  for(int ty = 0; ty < LCD_SIZE_Y; ty++) {
    for(int tx = 0; tx < LCD_SIZE_X; tx++) {
      lcdBuff[ty][tx] = ' ';
    }  
  }

  lcd.clear();
  lcd.setCursor(0, 0);
  x = y = 0;
}

// Clears line and sets cursor at the begging of cleared line
void lcdClearLine(int line) {
  if (line < 0 || line >= LCD_SIZE_Y) return;

  lcd.setCursor(0, line);
  for(int tx = 0; tx < LCD_SIZE_X; tx++) {
    lcdBuff[line][tx] = ' ';
    lcd.print(' ');
  }

  lcd.setCursor(0, line);
  x = 0;
  y = line;

  if (line == scrollerLine) {
    scrollerLine = -1;
  }
}

// Sets position of cursor
void lcdCursor(int _x, int _y) {
  x = _x;
  y = _y;

  lcd.setCursor(x, y);
}

void scrollLoop() {
  int maxScroll = constrain(scrollerLen - 16, 0, MAX_SCROLLER_LINE - 16);
  if(scrollX <= 0) scrollDir = true;
  else if(scrollX >= maxScroll) scrollDir = false;

  char buff[LCD_SIZE_X + 1];
  for(int i = 0; i < LCD_SIZE_X; i++) {
    buff[i] = scrollerBuff[i + scrollX];
  }
  buff[LCD_SIZE_X] = '\0';
  y = scrollerLine;
  printToScreen(buff, true, ALIGN_LEFT);

  if (maxScroll > 0) scrollX += scrollDir ? 1 : -1;
  else scrollX = 0;
}

void lcdScroller(int line, const char *str) {
  int strl = constrain(strlen(str), 0, MAX_SCROLLER_LINE);
  bool changed = line != scrollerLine;

  for(int i = 0; i < strl; i++) {
    if(scrollerBuff[i] != str[i]) {
      changed = true;
    }
  }

  if (changed) {
    scrollX = 0;
    scrollerLine = line;
    y = line;
    scrollDir = true;
    memcpy(scrollerBuff, str, strl + 1);
    char lineBuff[LCD_SIZE_X];
    memcpy(lineBuff, str, strl + 1);
    printToScreen(lineBuff, true, ALIGN_LEFT);
    scrollerLen = strl;
  }
}

/// @brief 
/// @param str string to print
/// @param fillBlank if string should be padded with spaces (blanks) to the end of screen
/// @param aligment text aligment (left/center/right)
void printToScreen(char* str, bool fillBlank = true, PrintAligment aligment = ALIGN_LEFT) {
  int strl = strlen(str);
  int leftOffset = 0;
  switch(aligment) {
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
  for(int i = 0; i < LCD_SIZE_X; i++) {
    if(fillBlank && ((i < leftOffset && aligment != ALIGN_NEXTTO) || i >= leftOffset + strl)) {
      lcdBuff[y][i] = ' ';
      lcd.setCursor(i, y);
      lcd.print(' ');

      continue;
    }

    if(i >= leftOffset && strI < strl) {
      if(lcdBuff[y][i] != str[strI]) {
        lcdBuff[y][i] = str[strI];
        lcd.setCursor(i, y);
        lcd.print(str[strI]);

      }

      strI++;
    }
  }

  x = leftOffset + strl;
  if (x >= LCD_SIZE_X) x = LCD_SIZE_X;
  lcd.setCursor(x, y);
}

void lcdPrintf(int line, bool fillBlank, PrintAligment aligment, const char *format, ...) {
    if (y < 0 || y >= LCD_SIZE_Y) return;
    
    va_list arg;
    va_start(arg, format);
    char temp[64];
    char* buffer = temp;
    size_t len = vsnprintf(temp, sizeof(temp), format, arg);
    va_end(arg);
    if (len > sizeof(temp) - 1) {
        buffer = new (std::nothrow) char[len + 1];
        if (!buffer) {
            return;
        }
        va_start(arg, format);
        vsnprintf(buffer, len + 1, format, arg);
        va_end(arg);
    }

    y = line;
    if (line == scrollerLine) scrollerLine = -1;

    if (len > LCD_SIZE_X) lcdScroller(line, buffer);
    else printToScreen(buffer, fillBlank, aligment);

    if (buffer != temp) {
        delete[] buffer;
    }
}

#endif
