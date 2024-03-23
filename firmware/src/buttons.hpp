#ifndef __BUTTONS_HPP__
#define __BUTTONS_HPP__

#include "defines.h"
#include "lcd.hpp"
#include "pins.h"
#include "state.hpp"
#include "translations.h"
#include <a_buttons.h>
#include <WiFiManager.h>

AButtons buttons;

void delegateButtonHold(int holdTime) {
  if (state.currentScene == SCENE_ERROR)
    return;
  if (holdTime > DELEGAT_BUTTON_HOLD_TIME)
    return;
  lockStateChange = true;

  int secs = ceilf((DELEGAT_BUTTON_HOLD_TIME - holdTime) / 1000.0);
  lcdPrintf(0, true, ALIGN_CENTER, TR_DELEGATE_HEADER);
  lcdPrintf(1, true, ALIGN_CENTER, TR_DELEGATE_COUNTDOWN, secs);
}

void delegateButtonCalled(Button &b) {
  if (state.currentScene == SCENE_ERROR)
    return;
  lcdPrintf(0, true, ALIGN_CENTER, TR_DELEGATE_CALLED_TOP);
  lcdPrintf(1, true, ALIGN_CENTER, TR_DELEGATE_CALLED_BOTTOM);

  stopInspection(); // stop inspection
  sendSolve(true); // send delegate request (TODO: maybe different method?)
}

void delegateButtonAfterRelease(Button &b) {
  if (state.currentScene != SCENE_ERROR)
    lcdClear();
  lockStateChange = false;
  stateHasChanged = true;
}

void penaltyButton(Button &b) {
  if (state.currentScene != SCENE_FINISHED_TIME)
    return;
  if (state.timeConfirmed)
    return;

  state.penalty =
      (state.penalty >= 16 || state.penalty == -1) ? 0 : state.penalty + 2;
  stateHasChanged = true;
}

void dnfButton(Button &b) {
  if (state.currentScene != SCENE_FINISHED_TIME)
    return;
  if (state.timeConfirmed)
    return;

  state.penalty = state.penalty == -1 ? 0 : -1;
  stateHasChanged = true; // refresh state
  b.disableAfterReleaseCbs = true;
}

void submitButton(Button &b) {
  if (state.currentScene == SCENE_ERROR) {
    lcdClear();
    state.currentScene = state.sceneBeforeError;
    stateHasChanged = true;

    return;
  }

  if (state.currentScene != SCENE_FINISHED_TIME)
    return;
  if (state.timeConfirmed)
    return;

  state.timeConfirmed = true;
  stateHasChanged = true;
}

void resetCompettorButton(Button &b) { resetSolveState(); }

void resetWifiButton(Button &b) {
  Logger.println("Resetting wifi settings!");
  WiFiManager wm;
  wm.resetSettings();
  delay(1000);
  ESP.restart();
}

void debugButton(Button &b) {
  logState();
  // state.competitorCardId = 3004425529;
  // startSolveSession(6969);
}

void inspectionButton(Button &b) {
  if (!state.useInspection) return;

  startInspection();
}

void buttonsInit() {
  size_t delegateBtn =
      buttons.addButton(BUTTON1, NULL, delegateButtonAfterRelease);
  buttons.addButtonReocCb(delegateBtn, 1000, delegateButtonHold);
  buttons.addButtonCb(delegateBtn, DELEGAT_BUTTON_HOLD_TIME, false,
                      delegateButtonCalled);

  size_t penaltyBtn = buttons.addButton(BUTTON2, NULL, NULL);
  buttons.addButtonCb(penaltyBtn, 0, true, penaltyButton);
  buttons.addButtonCb(penaltyBtn, DNF_BUTTON_HOLD_TIME, false, dnfButton);
  buttons.addButtonCb(penaltyBtn, RESET_WIFI_HOLD_TIME, false, resetWifiButton);

  size_t submitBtn = buttons.addButton(BUTTON3, NULL, NULL);
  buttons.addButtonCb(submitBtn, 0, true, submitButton);
  buttons.addButtonCb(submitBtn, RESET_COMPETITOR_HOLD_TIME, false,
                      resetCompettorButton);

  size_t inspectionBtn = buttons.addButton(BUTTON4, NULL, NULL);
  buttons.addButtonCb(inspectionBtn, 0, true, inspectionButton);

  size_t dbgBtn = buttons.addMultiButton({BUTTON1, BUTTON2}, NULL, debugButton);
}

#endif
