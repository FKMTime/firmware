#ifndef __LCD_HPP__
#define __LCD_HPP__

#define LCD_SIZE_X 16
#define LCD_SIZE_Y 2

#include "globals.hpp"


char lcdBuff[LCD_SIZE_Y][LCD_SIZE_X];
int x, y = 0;

bool stateHasChanged = true;
unsigned long lcdLastDraw = 0;

enum PrintAligment {
  ALIGN_LEFT = 0,
  ALIGN_CENTER = 1,
  ALIGN_RIGHT = 2,
  ALIGN_NEXTTO = 3 // ALIGN AFTER LAST TEXT
};

void lcdPrintf(int line, bool fillBlank, PrintAligment aligment, const char *format, ...);
void lcdClearLine(uint8_t line);

inline void lcdChange() {
  stateHasChanged = true;
}

inline void lcdLoop() {
  if (!stateHasChanged || millis() - lcdLastDraw < 50) return;
  stateHasChanged = false;

  if (!webSocket.isConnected()) {
    lcdPrintf(0, true, ALIGN_CENTER, "Server");
    lcdPrintf(1, true, ALIGN_CENTER, "Disconnected");
  } else if (state.finishedSolveTime > 0 && state.solverCardId > 0) { // after timer is stopped and solver scanned his card
    uint8_t minutes = state.finishedSolveTime / 60000;
    uint8_t seconds = (state.finishedSolveTime % 60000) / 1000;
    uint16_t ms = state.finishedSolveTime % 1000;

    lcdPrintf(0, true, ALIGN_LEFT, "%i:%02i.%03i", minutes, seconds, ms);
    if(state.timeOffset == -1) {
      lcdPrintf(0, false, ALIGN_RIGHT, " DNF");
    } else if (state.timeOffset > 0) {
      lcdPrintf(0, false, ALIGN_RIGHT, " +%d", state.timeOffset);
    }
    
    if (!state.timeConfirmed) {
      lcdPrintf(1, true, ALIGN_RIGHT, "Confirm the time");
    } else if (state.judgeCardId == 0) {
      lcdPrintf(1, true, ALIGN_RIGHT, "Awaiting judge");
    }
  } else if (!stackmat.connected()) {
    lcdPrintf(0, true, ALIGN_CENTER, "Stackmat");
    lcdPrintf(1, true, ALIGN_CENTER, "Disconnected");
  } else if (stackmat.state() == StackmatTimerState::ST_Running && state.solverCardId > 0) { // timer running and solver scanned his card
    lcdPrintf(0, true, ALIGN_CENTER, "%i:%02i.%03i", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
    lcdClearLine(1);
  } else if (state.solverCardId > 0) {
    lcdPrintf(0, true, ALIGN_CENTER, "Solver");
    lcdPrintf(1, true, ALIGN_CENTER, state.solverName.c_str());
  } else if (state.solverCardId == 0) {
    lcdPrintf(0, true, ALIGN_CENTER, "Stackmat");
    lcdPrintf(1, true, ALIGN_CENTER, "Awaiting solver");
  } else {
    lcdPrintf(0, true, ALIGN_CENTER, "Stackmat");
    lcdPrintf(1, true, ALIGN_CENTER, "Unhandled state!");
  }

  lcdLastDraw = millis();
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
void lcdClearLine(uint8_t line) {
  if (line < 0 || line >= LCD_SIZE_Y) return;

  lcd.setCursor(0, line);
  for(int tx = 0; tx < LCD_SIZE_X; tx++) {
    lcdBuff[line][tx] = ' ';
    lcd.print(' ');
  }

  lcd.setCursor(0, line);
  x = 0;
  y = line;
}

// Sets position of cursor
void lcdCursor(int _x, int _y) {
  x = _x;
  y = _y;

  lcd.setCursor(x, y);
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
  }
  if (leftOffset < 0) leftOffset = 0;

  int strI = 0;
  for(int i = 0; i < LCD_SIZE_X; i++) {
    if(fillBlank && (i < leftOffset || i >= leftOffset + strl)) {
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

  x += leftOffset + strl;
  if (x >= LCD_SIZE_X) x = LCD_SIZE_X - 1;
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
    printToScreen(buffer, fillBlank, aligment);

    if (buffer != temp) {
        delete[] buffer;
    }
}

#endif
