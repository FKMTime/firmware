#ifndef __BUTTONS_HPP__
#define __BUTTONS_HPP__

#include <a_buttons.h>
#include "pins.h"
AButtons buttons;

void delegateButtonHold3() {
    blockLcdChange(true);
    lcdPrintf(0, true, ALIGN_CENTER, "Delegate");
    lcdPrintf(1, true, ALIGN_CENTER, "In 3");
}

void delegateButtonHold2() {
    lcdPrintf(1, true, ALIGN_CENTER, "In 2");
}

void delegateButtonHold1() {
    lcdPrintf(1, true, ALIGN_CENTER, "In 1");
}

void delegateButtonCalled() {
    lcdPrintf(0, true, ALIGN_CENTER, "Delegate callled");
    lcdPrintf(1, true, ALIGN_CENTER, "Release button");
}

void delegateButtonAfterRelease() {
  blockLcdChange(false);
  lcdChange();
}

void buttonsInit() {
  size_t delegateBtn = buttons.addButton(BUTTON1, delegateButtonAfterRelease);
  buttons.addButtonCb(delegateBtn, 0, false, delegateButtonHold3);
  buttons.addButtonCb(delegateBtn, 1000, false, delegateButtonHold2);
  buttons.addButtonCb(delegateBtn, 2000, false, delegateButtonHold1);
  buttons.addButtonCb(delegateBtn, 3000, false, delegateButtonCalled);

//   size_t btn2 = buttons.addButton(BUTTON2);
//   buttons.addButtonCb(btn2, 1000, false, btnTest);
//   buttons.addButtonCb(btn2, 2000, false, btnTest2);
}

#endif