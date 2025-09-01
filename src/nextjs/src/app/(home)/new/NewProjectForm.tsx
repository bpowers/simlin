'use client';

import {
  Accordion,
  AccordionDetails,
  AccordionSummary,
  Button,
  Checkbox,
  Grid,
  InputAdornment,
  TextField,
  Typography,
} from '@mui/material';
import { ExpandMore } from '@mui/icons-material';
import { useActionState } from 'react';

import createProjectAction from '@/lib/createProjectAction';
import convertModelFileToBinary from '@/lib/convertModelFileToBinary';

export interface Props {
  userId: string;
}

export default function NewProjectForm({ userId }: Props) {
  async function clientAction(actionState: { errorMessage?: string; formData: FormData }, formData: FormData) {
    const modelFile = formData.get('model-file') as File | null;

    if (modelFile && modelFile.size !== 0) {
      const initialProjectBinary = await convertModelFileToBinary(modelFile);
      formData.set('model-file', new TextDecoder('utf-8').decode(initialProjectBinary));
    } else formData.delete('model-file');

    return createProjectAction(actionState, formData);
  }

  const [actionState, createProject, isPending] = useActionState(clientAction, { formData: new FormData() });

  return (
    <form action={createProject} className="TODO">
      <Typography variant="h2">Create a project</Typography>
      <Typography variant="subtitle1">A project holds models and data, along with simulation results.</Typography>
      <TextField
        autoFocus
        id="name"
        name="name"
        label="Project Name"
        type="text"
        fullWidth
        slotProps={{ input: { startAdornment: <InputAdornment position="start">{userId}/</InputAdornment> } }}
        required
        defaultValue={actionState.formData.get('name')}
      />
      <TextField
        id="description"
        name="description"
        label="Project Description"
        type="text"
        fullWidth
        defaultValue={actionState.formData.get('description')}
      />
      <Accordion>
        <AccordionSummary expandIcon={<ExpandMore />}>
          <Typography>Advanced</Typography>
        </AccordionSummary>
        <AccordionDetails>
          <Grid container spacing={10} justifyContent="center" alignItems="center">
            <Grid size={8}>
              <Typography>Use existing model</Typography>
            </Grid>
            <Grid size={4}>
              Select
              <input accept=".stmx,.itmx,.xmile,.mdl" id="model-file" name="model-file" type="file" />
            </Grid>
            <Grid size={12}>
              <Typography>
                <Checkbox
                  defaultChecked={actionState.formData.get('is-public') === 'true'}
                  name="is-public"
                  id="is-public"
                />
                Publicly accessible
              </Typography>
            </Grid>
          </Grid>
        </AccordionDetails>
      </Accordion>
      <Typography variant="subtitle2" style={{ whiteSpace: 'pre-wrap' }}>
        <b>{actionState.errorMessage}</b>
      </Typography>
      <Typography align="right">
        <Button disabled={isPending} type="submit" color="primary">
          Create
        </Button>
      </Typography>
    </form>
  );
}
