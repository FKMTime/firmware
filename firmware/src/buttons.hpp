#ifndef __BUTTONS_HPP__
#define __BUTTONS_HPP__

#include <a_buttons.h>
#include "pins.h"
#include "defines.h"
#include "translations.h"
#include "state.hpp"
#include "lcd.hpp"

AButtons buttons;

void delegateButtonHold(int holdTime) {
    if(holdTime > DELEGAT_BUTTON_HOLD_TIME) return;
    lockStateChange = true;

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
  lockStateChange = false;
  stateHasChanged = true;
}

void penaltyButton(Button &b) {
  if (state.currentScene != SCENE_FINISHED_TIME) return;
  if (state.timeConfirmed) return;

  state.penalty = (state.penalty >= 16 || state.penalty == -1) ? 0 : state.penalty + 2;
  stateHasChanged = true;
}

void dnfButton(Button &b) {
  if (state.currentScene != SCENE_FINISHED_TIME) return;
  if (state.timeConfirmed) return;

  state.penalty = state.penalty == -1 ? 0 : -1;
  stateHasChanged = true; // refresh state
  b.disableAfterReleaseCbs = true;
}

void submitButton(Button &b) {
  if (state.currentScene != SCENE_FINISHED_TIME) return;
  if (state.timeConfirmed) return;

  state.timeConfirmed = true;
  stateHasChanged = true;
}

void debugButton(Button &b) {
  Logger.printf("dbg here\n");
  startSolveSession(6969);
}

void buttonsInit() {
  size_t delegateBtn = buttons.addButton(BUTTON1, NULL, delegateButtonAfterRelease);
  buttons.addButtonReocCb(delegateBtn, 1000, delegateButtonHold);
  buttons.addButtonCb(delegateBtn, DELEGAT_BUTTON_HOLD_TIME, false, delegateButtonCalled);

  size_t penaltyBtn = buttons.addButton(BUTTON2, NULL, NULL);
  buttons.addButtonCb(penaltyBtn, 0, true, penaltyButton);
  buttons.addButtonCb(penaltyBtn, DNF_BUTTON_HOLD_TIME, false, dnfButton);

  size_t submitBtn = buttons.addButton(BUTTON3, NULL, NULL);
  buttons.addButtonCb(submitBtn, 0, true, submitButton);

  size_t dbgBtn = buttons.addMultiButton({BUTTON1, BUTTON2}, NULL, debugButton);
}

#endif
