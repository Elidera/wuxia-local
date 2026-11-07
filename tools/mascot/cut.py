# Cut the mascot out of a generated picture with a plain light background:
#   python3 cut.py original.png   → ../../assets/mascot/girl.png (+ check.png on a dark background)
# Then run prepare.py. Needs numpy, scipy, Pillow.
import numpy as np
from PIL import Image
from scipy import ndimage as nd
src = np.asarray(Image.open(__import__('sys').argv[1]).convert('RGB')).astype(np.float32)
pal = np.array([(252,241,234),(250,224,208),(229,201,187),(245,215,200),(248,232,222),(238,210,196)], np.float32)
d = np.min(np.linalg.norm(src[:,:,None,:]-pal[None,None,:,:], axis=3), axis=2)
bglike = d < 20
lab, n = nd.label(bglike)
border = set(np.unique(np.concatenate([lab[0],lab[-1],lab[:,0],lab[:,-1]]))) - {0}
bg = np.isin(lab, list(border))
fg = ~bg
# keep the biggest foreground blob (the girl, with tail); drops bubbles, coin, sparkles
lab2, n2 = nd.label(fg)
sizes = nd.sum(fg, lab2, range(1, n2+1))
keep = 1 + int(np.argmax(sizes))
mask = lab2 == keep
mask = nd.binary_fill_holes(mask)
# soft edge: alpha from distance to the background colour near the border
edge = mask & ~nd.binary_erosion(mask, iterations=2)
alpha = mask.astype(np.float32)
alpha[edge] = np.clip((d[edge]-8)/30, 0, 1)
rgba = np.dstack([src, alpha*255]).astype(np.uint8)
im = Image.fromarray(rgba, 'RGBA')
bbox = im.getbbox(); im = im.crop(bbox)
im.save('../../assets/mascot/girl.png')
print(bbox, im.size, n2, sorted(sizes)[-3:])
prev = Image.new('RGBA', im.size, (29,32,39,255)); prev.alpha_composite(im); prev.convert('RGB').save('check.png')
