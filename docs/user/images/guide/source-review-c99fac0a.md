# Graphics capture source review for c99fac0a

Retained the published Graphics image after comparing it with a new game-free capture at the same 1280×720 size. The visible rows, values, selected help and navigation match. The version in the banner differs.

The capture now links `game/menu/rows.c` and passes its callback through the fixture's host. The Screen capture and fixture assertions verified that Ultrawide menus and Field of view are available from the live menu's row definitions. The manifest records the updated compiler inputs.
