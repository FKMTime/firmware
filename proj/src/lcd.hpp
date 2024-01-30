#ifndef __LCD_HPP__
#define __LCD_HPP__

#define LCD_SIZE_X 16
#define LCD_SIZE_Y 2

#include "globals.hpp"

char lcdBuff[LCD_SIZE_Y][LCD_SIZE_X];
uint8_t x, y = 0;

bool stateHasChanged = true;
unsigned long lcdLastDraw = 0;

enum PrintAligment {
  ALIGN_LEFT = 0,
  ALIGN_CENTER = 1,
  ALIGN_RIGHT = 2
};

inline void lcdChange() {
  stateHasChanged = true;
}

inline void lcdLoop() {
  if (!stateHasChanged || millis() - lcdLastDraw < 50) return;
  stateHasChanged = false;

  lcd.clear();
  lcd.setCursor(0, 0);
  if (!webSocket.isConnected()) {
    lcd.printf("     Server     ");
    lcd.setCursor(0, 1);
    lcd.print("  Disconnected  ");
  } else if (state.finishedSolveTime > 0 && state.solverCardId > 0) { // after timer is stopped and solver scanned his card
    uint8_t minutes = state.finishedSolveTime / 60000;
    uint8_t seconds = (state.finishedSolveTime % 60000) / 1000;
    uint16_t ms = state.finishedSolveTime % 1000;

    lcd.printf("%i:%02i.%03i", minutes, seconds, ms);
    if(state.timeOffset == -1) {
      lcd.printf(" DNF");
    } else if (state.timeOffset > 0) {
      lcd.printf(" +%d", state.timeOffset);
    }
    
    if (!state.timeConfirmed) {
      lcd.setCursor(0, 1);
      lcd.printf("Confirm the time");
    } else if (state.judgeCardId == 0) {
      lcd.setCursor(0, 1);
      lcd.printf("Awaiting judge");
    }
  } else if (!stackmat.connected()) {
    lcd.printf("    Stackmat    ");
    lcd.setCursor(0, 1);
    lcd.print("  Disconnected  ");
  } else if (stackmat.state() == StackmatTimerState::ST_Running && state.solverCardId > 0) { // timer running and solver scanned his card
    lcd.printf("%i:%02i.%03i", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
  } else if (state.solverCardId > 0) {
    lcd.printf("     Solver     ");
    lcd.setCursor(0, 1);
    lcd.printf(centerString(state.solverName, 16).c_str());
  } else if (state.solverCardId == 0) {
    lcd.printf("    Stackmat    ");
    lcd.setCursor(0, 1);
    lcd.printf("Awaiting solver");
  } else {
    lcd.printf("    Stackmat    ");
    lcd.setCursor(0, 1);
    lcd.printf("Unhandled state!");
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
void lcdCursor(uint8_t _x, uint8_t _y) {
  x = _x;
  y = _y;

  lcd.setCursor(x, y);
}

void lcdPrintf(const char *format, ...) {
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

    // buff here


    if (buffer != temp) {
        delete[] buffer;
    }
}

/// @brief 
/// @param str string to print
/// @param fillBlank if string should be padded with spaces (blanks) to the end of screen
/// @param aligment text aligment (left/center/right)
void printToScreen(char* str, bool fillBlank = true, PrintAligment aligment = ALIGN_LEFT) {
  char tmpBuff[LCD_SIZE_X];
  size_t strl = strlen(str);

  int leftOffset = 0;
  switch(aligment) {
    case 0:
      leftOffset = 0;
      break;
    case 1:
      leftOffset = (LCD_SIZE_X - strl) / 2;
      break;
    case 2:
      leftOffset = LCD_SIZE_X - strl;
      break;
  }
  if (leftOffset < 0) leftOffset = 0;

  for(int i = 0; i < LCD_SIZE_X; i++) {
    if (fillBlank) lcd[y][i] = ' ';

    if(i + leftOffset < LCD_SIZE_X) {
      if(strl > i) lcd[y][i + leftOffset] = str[i];
    }
  }

  // lcd.print(tmpBuff);
}

#endif