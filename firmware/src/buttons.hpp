#ifndef __BUTTONS_HPP__
#define __BUTTONS_HPP__

#include "a_buttons.h"

extern AButtons buttons;
void delegateButtonHold(int holdTime);
void delegateButtonCalled(Button &b);
void delegateButtonAfterRelease(Button &b);
void penaltyButton(Button &b);
void dnfButton(Button &b);
void submitButton(Button &b);
void resetCompetitorButton(Button &b);
void resetWifiButton(Button &b);
void debugButton(Button &b);
void calibrationButton(Button &b);
void inspectionButton(Button &b);
void buttonsInit();

#endif
