#ifndef __BUTTONS_HPP__
#define __BUTTONS_HPP__

#include <a_buttons.h>
#include "pins.h"
#include "defines.h"
#include "translations.h"
#include "state.hpp"

AButtons buttons;

void delegateButtonHold(int holdTime) {
    if(holdTime > DELEGAT_BUTTON_HOLD_TIME) return;
    blockLcdChange(true);

    int secs = ceilf((DELEGAT_BUTTON_HOLD_TIME - holdTime) / 1000.0);
    lcdPrintf(0, true, ALIGN_CENTER, TR_DELEGATE_HEADER);
    lcdPrintf(1, true, ALIGN_CENTER, TR_DELEGATE_COUNTDOWN, secs);
}

void delegateButtonCalled(Button &b) {
    lcdPrintf(0, true, ALIGN_CENTER, TR_DELEGATE_CALLED_TOP);
    lcdPrintf(1, true, ALIGN_CENTER, TR_DELEGATE_CALLED_BOTTOM);
}

void delegateButtonAfterRelease(Button &b) {
  lcdClear();

  blockLcdChange(false);
  lcdChange();
}

void penaltyButton(Button &b) {
  if (state.currentScene != SCENE_WAITING_FOR_COMPETITOR) return;
  if (state.timeConfirmed) return;

  state.penalty = (state.penalty >= 16 || state.penalty == -1) ? 0 : state.penalty + 2;
  stateHasChanged = true;
}

void dnfButton(Button &b) {
  state.penalty = state.penalty == -1 ? 0 : -1;
  stateHasChanged = true; // refresh state
  b.disableAfterReleaseCbs = true;
}

void debugButton(Button &b) {
  Logger.printf("dbg here\n");
}

void buttonsInit() {
  size_t delegateBtn = buttons.addButton(BUTTON1, NULL, delegateButtonAfterRelease);
  buttons.addButtonReocCb(delegateBtn, 1000, delegateButtonHold);
  buttons.addButtonCb(delegateBtn, DELEGAT_BUTTON_HOLD_TIME, false, delegateButtonCalled);

  size_t penaltyBtn = buttons.addButton(BUTTON2, NULL, NULL);
  buttons.addButtonCb(penaltyBtn, 0, true, penaltyButton);
  buttons.addButtonCb(penaltyBtn, DNF_BUTTON_HOLD_TIME, false, dnfButton);

  size_t dbgBtn = buttons.addMultiButton({BUTTON1, BUTTON2}, NULL, debugButton);
}

#endif
