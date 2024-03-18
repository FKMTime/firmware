#ifndef __BUTTONS_HPP__
#define __BUTTONS_HPP__

#include <a_buttons.h>
#include "pins.h"
AButtons buttons;

void delegateButtonHold(int holdTime) {
    if(holdTime > 3000) return;
    blockLcdChange(true);

    int secs = ceilf((3000 - holdTime) / 1000.0);
    lcdPrintf(0, true, ALIGN_CENTER, "Delegate");
    lcdPrintf(1, true, ALIGN_CENTER, "In %d", secs);
}

void delegateButtonCalled() {
    lcdPrintf(0, true, ALIGN_CENTER, "Delegate callled");
    lcdPrintf(1, true, ALIGN_CENTER, "Release button");
}

void delegateButtonAfterRelease() {
  lcdClear();

  blockLcdChange(false);
  lcdChange();
}

void testBtn2Hold(int holdTime) {
    blockLcdChange(true);

    lcdPrintf(0, true, ALIGN_CENTER, "Test Holding");
    lcdPrintf(1, true, ALIGN_CENTER, "%d", holdTime);
}

void testBtn2AfterRelease() {
  lcdClear();

  blockLcdChange(false);
  lcdChange();
}

void debugAfterRelease() {
  Logger.printf("dbg:\n");
  for(int y = 0; y < LCD_SIZE_Y; y++) {
    Logger.printf("y: %d | %s\n", y, lcdBuff[y]);
    Logger.printf("y2: %d | %s\n", y, shownBuff[y]);
  }
}

void buttonsInit() {
  size_t delegateBtn = buttons.addButton(BUTTON1, NULL, delegateButtonAfterRelease);
  buttons.addButtonReocCb(delegateBtn, 1000, delegateButtonHold);
  buttons.addButtonCb(delegateBtn, 3000, false, delegateButtonCalled);

  size_t btn2 = buttons.addButton(BUTTON2, NULL, testBtn2AfterRelease);
  buttons.addButtonReocCb(btn2, 0, testBtn2Hold);

  size_t dbgBtn = buttons.addMultiButton({BUTTON1, BUTTON2}, NULL, debugAfterRelease);

//   size_t btn2 = buttons.addButton(BUTTON2);
//   buttons.addButtonCb(btn2, 1000, false, btnTest);
//   buttons.addButtonCb(btn2, 2000, false, btnTest2);
}

#endif
