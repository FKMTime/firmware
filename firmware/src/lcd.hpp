#ifndef __LCD_HPP__
#define __LCD_HPP__

#include "defines.h"
#define MAX_SCROLLER_LINE 64
#define SCROLLER_SPEED 500
#define SCROLLER_SETBACK 1000

extern char shownBuff[LCD_SIZE_Y][LCD_SIZE_X];

extern bool lcdHasChanged;
extern unsigned long lcdLastDraw;

enum PrintAligment {
  ALIGN_LEFT = 0,
  ALIGN_CENTER = 1,
  ALIGN_RIGHT = 2,
  ALIGN_NEXTTO = 3 // ALIGN AFTER LAST TEXT
};

void waitForLock();
void lcdInit();
void lcdLoop();

void printLcdBuff(bool force = false);

/// Clears screen and sets cursor on (0, 0)
void lcdClear();

/// Clears line and sets cursor at the begging of cleared line
void lcdClearLine(int line);
void scrollLoop();
void lcdScroller(int line, const char *str);

/// @brief
/// @param str string to print
/// @param fillBlank if string should be padded with spaces (blanks) to the end
/// of screen
/// @param aligment text aligment (left/center/right)
/// @param forceUnlock set to true if you dont want to lock
void printToScreen(char *str, bool fillBlank = true,
                   PrintAligment aligment = ALIGN_LEFT,
                   bool forceUnlock = false);

void lcdPrintf(int line, bool fillBlank, PrintAligment aligment,
               const char *format, ...);

#endif
